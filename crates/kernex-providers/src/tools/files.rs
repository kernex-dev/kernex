//! The in-process file tools: `read`, `write`, `edit`, `grep` and `glob`.
//!
//! These run inside the host process, so each path goes through the sandbox
//! crate's tool checks first: a tool reaches the workspace and the host's
//! explicit extra paths, nothing else. A file is opened at the resolved path
//! the check returned, without following a symlink at its last component.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::debug;

use super::{
    truncate_output, which_exists, ToolExecutor, ToolResult, MAX_GREP_PATTERN_LEN, MAX_READ_OUTPUT,
};

fn error(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: true,
    }
}

fn ok(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: false,
    }
}

/// The one answer for every refused path. It names the path as the caller
/// gave it and not the rule that matched.
fn denied(path: &str) -> ToolResult {
    error(format!(
        "Access denied: {path} is outside the tool scope or protected"
    ))
}

/// Open options that refuse a symlink at the last path component.
fn open_options(configure: impl FnOnce(&mut std::fs::OpenOptions)) -> tokio::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    configure(&mut options);
    kernex_sandbox::no_follow(&mut options);
    tokio::fs::OpenOptions::from(options)
}

/// Open `path` for reading. Only a regular file is accepted: a FIFO or a
/// device would block the read or never end.
async fn open_regular(path: &Path) -> std::io::Result<tokio::fs::File> {
    let meta = tokio::fs::symlink_metadata(path).await?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    open_options(|o| {
        o.read(true);
    })
    .open(path)
    .await
}

async fn read_file(path: &Path) -> std::io::Result<String> {
    let mut content = String::new();
    open_regular(path)
        .await?
        .read_to_string(&mut content)
        .await?;
    Ok(content)
}

/// Read at most `limit` bytes of `path` as text, plus the few bytes that
/// tell a longer file from one that ends there.
async fn read_file_head(path: &Path, limit: usize) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    open_regular(path)
        .await?
        .take(limit as u64 + 4)
        .read_to_end(&mut bytes)
        .await?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        // The cut fell inside a character: keep what is whole.
        Err(e) if e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        )),
    }
}

async fn write_file(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file = open_options(|o| {
        o.write(true).create(true).truncate(true);
    })
    .open(path)
    .await?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await
}

/// Whether one line of search output names a path under `excluded`.
fn names_excluded(line: &str, excluded: &[PathBuf]) -> bool {
    // `grep` reports a binary match and its own errors with a prefix.
    let line = ["Binary file ", "grep: ", "rg: ", "find: "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .unwrap_or(line);
    excluded.iter().any(|path| {
        let prefix = path.to_string_lossy();
        line.strip_prefix(prefix.as_ref()).is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with('/')
                || rest.starts_with(':')
                || rest.starts_with(' ')
        })
    })
}

/// Drop the lines of search output that name a protected path.
fn without_excluded(output: &str, excluded: &[PathBuf]) -> String {
    if excluded.is_empty() {
        return output.to_string();
    }
    output
        .lines()
        .filter(|line| !names_excluded(line, excluded))
        .fold(String::new(), |mut kept, line| {
            kept.push_str(line);
            kept.push('\n');
            kept
        })
}

impl ToolExecutor {
    /// Resolve the search root of `grep` or `glob`: the `path` argument, or
    /// the workspace. Returns the root and the protected paths below it.
    fn search_root(&self, args: &Value) -> Result<(PathBuf, Vec<PathBuf>), ToolResult> {
        let given = args.get("path").and_then(|v| v.as_str());
        let requested = given
            .map(|p| self.resolve_path(p))
            .unwrap_or_else(|| self.workspace_path.clone());
        let root = kernex_sandbox::tool_read_path(&requested, &self.scope, &self.sandbox_profile)
            .map_err(|_| denied(given.unwrap_or(".")))?;
        let excluded =
            kernex_sandbox::tool_search_exclusions(&root, &self.scope, &self.sandbox_profile);
        Ok((root, excluded))
    }

    pub(super) async fn exec_read(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if path_str.is_empty() {
            return error("Error: 'file_path' parameter is required");
        }

        // Relative paths are joined to the workspace before the check.
        let requested = self.resolve_path(path_str);
        let Ok(path) =
            kernex_sandbox::tool_read_path(&requested, &self.scope, &self.sandbox_profile)
        else {
            return denied(path_str);
        };

        debug!("tool/read: {}", path.display());

        match read_file_head(&path, MAX_READ_OUTPUT).await {
            Ok(content) => ok(truncate_output(&content, MAX_READ_OUTPUT)),
            Err(e) => error(format!("Error reading {path_str}: {e}")),
        }
    }

    pub(super) async fn exec_write(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if path_str.is_empty() {
            return error("Error: 'file_path' parameter is required");
        }

        let requested = self.resolve_path(path_str);
        let Ok(path) =
            kernex_sandbox::tool_write_path(&requested, &self.scope, &self.sandbox_profile)
        else {
            return denied(path_str);
        };

        debug!("tool/write: {}", path.display());

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return error(format!("Failed to create parent directory: {e}"));
            }
        }

        match write_file(&path, content).await {
            Ok(()) => ok(format!("Wrote {} bytes to {path_str}", content.len())),
            Err(e) => error(format!("Error writing {path_str}: {e}")),
        }
    }

    pub(super) async fn exec_edit(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let old_string = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_string = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if path_str.is_empty() {
            return error("Error: 'file_path' parameter is required");
        }
        if old_string.is_empty() {
            return error("Error: 'old_string' parameter is required");
        }

        let requested = self.resolve_path(path_str);
        let Ok(path) =
            kernex_sandbox::tool_write_path(&requested, &self.scope, &self.sandbox_profile)
        else {
            return denied(path_str);
        };

        debug!("tool/edit: {}", path.display());

        let content = match read_file(&path).await {
            Ok(c) => c,
            Err(e) => return error(format!("Error reading {path_str}: {e}")),
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            return error("Error: old_string not found in file");
        }

        let new_content = content.replacen(old_string, new_string, 1);
        match write_file(&path, &new_content).await {
            Ok(()) => ok(format!(
                "Edited {path_str} ({count} occurrence(s) of pattern, replaced first)"
            )),
            Err(e) => error(format!("Error writing {path_str}: {e}")),
        }
    }

    /// Run ripgrep (or grep) to search file contents.
    ///
    /// Arguments are passed as a pre-split argv array, with no shell, and the
    /// pattern follows `-e` so that one beginning with `-` is still a pattern
    /// and never an option.
    pub(super) async fn exec_grep(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return error("Error: 'pattern' parameter is required");
        }
        // Length cap on the pattern to limit ReDoS surface. The 30 s
        // timeout is the ultimate safety net, but a tool-using model that
        // sets `pattern` to a pathological backtracking regex (e.g.
        // `(a+)+$`) on a large repo can still burn 30 s of CPU per call.
        // 1024 chars is generous for any realistic search and shrinks the
        // attack window.
        if pattern.len() > MAX_GREP_PATTERN_LEN {
            return error(format!(
                "Error: 'pattern' exceeds {MAX_GREP_PATTERN_LEN}-char limit"
            ));
        }

        let (search_path, excluded) = match self.search_root(args) {
            Ok(found) => found,
            Err(refused) => return refused,
        };
        let glob_pattern = args.get("glob").and_then(|v| v.as_str());

        debug!(
            "tool/grep: pattern={pattern} path={}",
            search_path.display()
        );

        // Prefer `rg` (ripgrep) when available; fall back to `grep -r`.
        let (cmd_name, mut argv): (&str, Vec<String>) = if which_exists("rg") {
            let mut a = vec![
                "--line-number".to_string(),
                "--color=never".to_string(),
                // A root that is a single file would otherwise print no
                // path, and the exclusion filter reads the path.
                "--with-filename".to_string(),
                // Cap the compiled regex DFA size to 16 MiB. Without this,
                // ripgrep allocates up to 500 MiB by default for complex
                // patterns. 16 MiB is plenty for legitimate searches and
                // bounds the worst-case memory footprint.
                "--regex-size-limit".to_string(),
                "16M".to_string(),
            ];
            if let Some(g) = glob_pattern {
                a.push(format!("--glob={g}"));
            }
            ("rg", a)
        } else {
            let mut a = vec![
                "-r".to_string(),
                "-n".to_string(),
                "-H".to_string(),
                "--include=*".to_string(),
            ];
            if let Some(g) = glob_pattern {
                // grep --include only supports simple globs; best-effort.
                a.push(format!("--include={g}"));
            }
            ("grep", a)
        };
        argv.push("-e".to_string());
        argv.push(pattern.to_string());
        argv.push("--".to_string());
        argv.push(search_path.to_string_lossy().to_string());

        let mut cmd = match kernex_sandbox::try_protected_command(
            cmd_name,
            &self.scope,
            &self.sandbox_profile,
        ) {
            Ok(c) => c,
            Err(e) => return error(format!("Refusing to run {cmd_name}: {e}")),
        };
        cmd.args(&argv);
        cmd.kill_on_drop(true);

        match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
            Ok(Ok(output)) => {
                let stdout = without_excluded(&String::from_utf8_lossy(&output.stdout), &excluded);
                // grep/rg exit code 1 = no matches (not an error for us).
                if stdout.is_empty() && !output.status.success() {
                    // A protected path the search could not enter is not
                    // an error to report, and its name is not repeated.
                    let stderr =
                        without_excluded(&String::from_utf8_lossy(&output.stderr), &excluded);
                    if stderr.is_empty() {
                        ok("(no matches found)")
                    } else {
                        error(format!("grep error: {stderr}"))
                    }
                } else if stdout.is_empty() {
                    ok("(no matches found)")
                } else {
                    ok(truncate_output(&stdout, MAX_READ_OUTPUT))
                }
            }
            Ok(Err(e)) => error(format!("grep execution failed: {e}")),
            Err(_) => error("grep timed out after 30s"),
        }
    }

    /// Walk a directory tree matching files against a glob pattern.
    pub(super) async fn exec_glob(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return error("Error: 'pattern' parameter is required");
        }

        let (base, excluded) = match self.search_root(args) {
            Ok(found) => found,
            Err(refused) => return refused,
        };

        debug!("tool/glob: pattern={pattern} base={}", base.display());

        // Shell out to `find` with safe pre-split argv (no shell interpolation).
        // Translate glob to find -name; for recursive patterns (**) use -path.
        let (flag, pat) = if pattern.contains('/') || pattern.contains("**") {
            ("-path", format!("./{pattern}"))
        } else {
            ("-name", pattern.to_string())
        };

        let mut cmd =
            match kernex_sandbox::try_protected_command("find", &self.scope, &self.sandbox_profile)
            {
                Ok(c) => c,
                Err(e) => return error(format!("Refusing to run find: {e}")),
            };
        cmd.arg(base.as_os_str())
            .arg(flag)
            .arg(&pat)
            .arg("-not")
            .arg("-path")
            .arg("*/.git/*");
        cmd.kill_on_drop(true);

        match tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await {
            Ok(Ok(output)) => {
                let stdout = without_excluded(&String::from_utf8_lossy(&output.stdout), &excluded);
                if stdout.is_empty() {
                    ok("(no files matched)")
                } else {
                    ok(truncate_output(&stdout, MAX_READ_OUTPUT))
                }
            }
            Ok(Err(e)) => error(format!("glob execution failed: {e}")),
            Err(_) => error("glob timed out after 15s"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A workspace with a neighbour directory it must not reach.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            std::fs::create_dir_all(root.join("ws/src")).unwrap();
            std::fs::create_dir_all(root.join("ws/secrets")).unwrap();
            std::fs::create_dir_all(root.join("outside")).unwrap();
            std::fs::write(root.join("ws/src/a.rs"), "fn needle() {}\n").unwrap();
            std::fs::write(root.join("ws/secrets/token"), "needle-secret\n").unwrap();
            std::fs::write(root.join("outside/secret.txt"), "needle-outside\n").unwrap();
            Self { _dir: dir, root }
        }

        fn path(&self, rel: &str) -> String {
            self.root.join(rel).to_string_lossy().into_owned()
        }

        fn executor(&self, profile: kernex_sandbox::SandboxProfile) -> ToolExecutor {
            let dirs = crate::SandboxDirs::new().with_data_dir(self.root.join("state"));
            ToolExecutor::new(self.root.join("ws"))
                .with_sandbox_dirs(&dirs)
                .with_sandbox_profile(profile)
        }

        fn plain(&self) -> ToolExecutor {
            self.executor(kernex_sandbox::SandboxProfile::new())
        }
    }

    const DENIED: &str = "is outside the tool scope or protected";

    #[tokio::test]
    async fn test_read_outside_the_workspace_is_refused() {
        let fx = Fixture::new();
        let tools = fx.plain();
        let inside = tools.exec_read(&json!({"file_path": "src/a.rs"})).await;
        assert!(!inside.is_error, "{}", inside.content);

        for path in [fx.path("outside/secret.txt"), "/etc/hosts".to_string()] {
            let result = tools.exec_read(&json!({"file_path": path})).await;
            assert!(result.is_error && result.content.contains(DENIED));
            assert!(!result.content.contains("needle"));
        }
        // A path that does not exist gets the same answer as one that does.
        let missing = tools
            .exec_read(&json!({"file_path": fx.path("outside/missing.txt")}))
            .await;
        assert!(missing.content.contains(DENIED), "{}", missing.content);
    }

    #[tokio::test]
    async fn test_read_reaches_skills_and_caps_a_large_file() {
        let fx = Fixture::new();
        let tools = fx.plain();
        std::fs::create_dir_all(fx.root.join("state/skills/review")).unwrap();
        std::fs::write(fx.root.join("state/skills/review/SKILL.md"), "skill body").unwrap();
        std::fs::write(fx.root.join("state/memory.db"), "db").unwrap();
        let skill = tools
            .exec_read(&json!({"file_path": fx.path("state/skills/review/SKILL.md")}))
            .await;
        assert_eq!(skill.content, "skill body");
        let db = tools
            .exec_read(&json!({"file_path": fx.path("state/memory.db")}))
            .await;
        assert!(db.is_error && db.content.contains(DENIED));

        // A file far over the output cap is read up to the cap; the cut may
        // fall inside a multi-byte character.
        std::fs::write(fx.root.join("ws/big.txt"), "\u{e9}".repeat(MAX_READ_OUTPUT)).unwrap();
        let big = tools.exec_read(&json!({"file_path": "big.txt"})).await;
        assert!(!big.is_error, "{}", big.content);
        assert!(big.content.len() < MAX_READ_OUTPUT + 200);

        // A directory is not a regular file.
        let dir = tools.exec_read(&json!({"file_path": "src"})).await;
        assert!(dir.is_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_read_of_a_fifo_returns_instead_of_blocking() {
        let fx = Fixture::new();
        let tools = fx.plain();
        let fifo = fx.root.join("ws/pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tools.exec_read(&json!({"file_path": "pipe"})),
        )
        .await
        .expect("read of a FIFO blocked");
        assert!(read.is_error && read.content.contains("not a regular file"));
    }

    #[tokio::test]
    async fn test_read_paths_extend_reads_and_not_writes() {
        let fx = Fixture::new();
        let profile =
            kernex_sandbox::SandboxProfile::new().with_read_paths(vec![fx.root.join("outside")]);
        let tools = fx.executor(profile);
        let path = fx.path("outside/secret.txt");
        let read = tools.exec_read(&json!({"file_path": path})).await;
        assert!(!read.is_error, "{}", read.content);
        let write = tools
            .exec_write(&json!({"file_path": path, "content": "x"}))
            .await;
        assert!(write.is_error && write.content.contains(DENIED));
        assert_eq!(
            std::fs::read_to_string(fx.root.join("outside/secret.txt")).unwrap(),
            "needle-outside\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_write_through_a_symlink_that_leaves_the_workspace_is_refused() {
        let fx = Fixture::new();
        let tools = fx.plain();
        std::os::unix::fs::symlink(fx.root.join("outside/secret.txt"), fx.root.join("ws/file"))
            .unwrap();
        std::os::unix::fs::symlink(fx.root.join("outside"), fx.root.join("ws/dir")).unwrap();
        std::os::unix::fs::symlink(fx.root.join("outside/new.txt"), fx.root.join("ws/dangling"))
            .unwrap();

        for path in ["file", "dir/secret.txt", "dir/created.txt", "dangling"] {
            let write = tools
                .exec_write(&json!({"file_path": path, "content": "overwritten"}))
                .await;
            assert!(write.is_error && write.content.contains(DENIED), "{path}");
            let edit = tools
                .exec_edit(&json!({"file_path": path, "old_string": "needle", "new_string": "x"}))
                .await;
            assert!(edit.is_error && edit.content.contains(DENIED), "{path}");
            let read = tools.exec_read(&json!({"file_path": path})).await;
            assert!(read.is_error && read.content.contains(DENIED), "{path}");
        }
        assert_eq!(
            std::fs::read_to_string(fx.root.join("outside/secret.txt")).unwrap(),
            "needle-outside\n"
        );
        assert!(!fx.root.join("outside/created.txt").exists());
        assert!(!fx.root.join("outside/new.txt").exists());
    }

    #[tokio::test]
    async fn test_write_and_edit_work_inside_the_workspace() {
        let fx = Fixture::new();
        let tools = fx.plain();
        let write = tools
            .exec_write(&json!({"file_path": "new/dir/file.txt", "content": "one two"}))
            .await;
        assert!(!write.is_error, "{}", write.content);
        let edit = tools
            .exec_edit(
                &json!({"file_path": "new/dir/file.txt", "old_string": "two", "new_string": "2"}),
            )
            .await;
        assert!(!edit.is_error, "{}", edit.content);
        assert_eq!(
            std::fs::read_to_string(fx.root.join("ws/new/dir/file.txt")).unwrap(),
            "one 2"
        );
    }

    #[tokio::test]
    async fn test_grep_pattern_that_looks_like_an_option_is_a_pattern() {
        let fx = Fixture::new();
        let tools = fx.plain();
        let marker = fx.root.join("ws/marker");
        // With ripgrep, `--pre=<program>` runs a program on every file. As a
        // pattern it is plain text to search for.
        let script = fx.root.join("ws/pre.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let pattern = format!("--pre={}", script.display());
        std::fs::write(
            fx.root.join("ws/notes.txt"),
            format!("see {pattern} here\n"),
        )
        .unwrap();

        let result = tools.exec_grep(&json!({"pattern": pattern})).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("notes.txt"), "{}", result.content);
        assert!(!marker.exists(), "the pattern was run as a preprocessor");
    }

    #[tokio::test]
    async fn test_grep_and_glob_stay_inside_the_scope() {
        let fx = Fixture::new();
        let blocked = kernex_sandbox::SandboxProfile::new()
            .with_blocked_paths(vec![fx.root.join("ws/secrets")]);
        let tools = fx.executor(blocked);

        // A root outside the workspace is refused.
        for root in [
            fx.path("outside"),
            "/etc".to_string(),
            fx.path("ws/secrets"),
        ] {
            let grep = tools
                .exec_grep(&json!({"pattern": "needle", "path": root}))
                .await;
            assert!(grep.is_error && grep.content.contains(DENIED), "{root}");
            let glob = tools
                .exec_glob(&json!({"pattern": "*", "path": root}))
                .await;
            assert!(glob.is_error && glob.content.contains(DENIED), "{root}");
        }

        // A protected path below the root is left out of the results.
        let grep = tools.exec_grep(&json!({"pattern": "needle"})).await;
        assert!(grep.content.contains("a.rs"), "{}", grep.content);
        assert!(!grep.content.contains("needle-secret"), "{}", grep.content);
        assert!(!grep.content.contains("needle-outside"), "{}", grep.content);
        let glob = tools.exec_glob(&json!({"pattern": "*"})).await;
        assert!(glob.content.contains("a.rs"), "{}", glob.content);
        assert!(!glob.content.contains("token"), "{}", glob.content);
    }

    #[test]
    fn test_without_excluded_drops_only_protected_paths() {
        let excluded = vec![PathBuf::from("/ws/secrets"), PathBuf::from("/ws/a.db")];
        let output = "/ws/src/a.rs:1:x\n/ws/secrets/token:1:y\n/ws/secrets\n/ws/a.db:3:z\n\
                      /ws/secrets-public/b:1:w\n/ws/a.db.md:1:v\n";
        assert_eq!(
            without_excluded(output, &excluded),
            "/ws/src/a.rs:1:x\n/ws/secrets-public/b:1:w\n/ws/a.db.md:1:v\n"
        );
        assert_eq!(without_excluded(output, &[]), output);
        let noise =
            "Binary file /ws/secrets/bin matches\ngrep: /ws/secrets: Operation not permitted\n";
        assert_eq!(without_excluded(noise, &excluded), "");
    }
}
