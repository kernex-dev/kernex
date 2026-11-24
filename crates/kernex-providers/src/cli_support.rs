//! Pieces shared by the providers that run an agent CLI as a subprocess
//! (`claude-code` and `opencode`).
//!
//! The CLI is the provider: it runs its own tools and starts its own MCP
//! servers inside the one sandbox the runtime puts around it. These helpers
//! decide what that sandbox is rooted in and which declared servers the CLI
//! may start.

use std::path::{Path, PathBuf};

use kernex_core::context::McpServer;

use crate::mcp_client::WORKSPACE_PLACEHOLDER;

/// The directory a CLI call works in.
///
/// A configured working directory is used as given. Without one, each call
/// gets a new empty directory under the system temp dir, removed when the
/// call ends, so the CLI never runs outside the sandbox for lack of a
/// workspace.
pub(crate) enum CliWorkspace {
    Configured(PathBuf),
    Temporary(tempfile::TempDir),
}

impl CliWorkspace {
    /// The configured directory, or a new temporary one.
    pub(crate) fn for_call(configured: Option<&Path>) -> std::io::Result<Self> {
        match configured {
            Some(dir) => Ok(Self::Configured(dir.to_path_buf())),
            None => tempfile::Builder::new()
                .prefix("kernex-cli-")
                .tempdir()
                .map(Self::Temporary),
        }
    }

    /// The directory itself.
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Configured(dir) => dir,
            Self::Temporary(dir) => dir.path(),
        }
    }
}

/// Whether an allowlist (`None` = every tool) admits the tools of the MCP
/// server `name`: `mcp__name` or `mcp__name__<tool>`, with or without a
/// pattern suffix.
pub(crate) fn allowlist_names_server(allowed: Option<&[String]>, name: &str) -> bool {
    let Some(list) = allowed else {
        return true;
    };
    list.iter().any(|entry| {
        let entry = entry.split('(').next().unwrap_or(entry).trim();
        entry
            .strip_prefix("mcp__")
            .map(|rest| rest.split_once("__").map_or(rest, |(server, _)| server))
            .is_some_and(|server| server == name)
    })
}

/// The declared MCP servers a CLI may start, ready to write into its config.
///
/// A server is left out when its name is not plain ASCII letters, digits,
/// `_` and `-` (the CLI builds permission rules from it), when its command
/// is relative or not in a non-empty declared allow-list, when the
/// allowlist names none of its tools, or when it does not declare network
/// access: the CLI starts it inside the CLI's own sandbox, which has the
/// network, so a server that asked for none could not be held to that.
/// `{workspace}` in an argument becomes the workspace path, and
/// dynamic-linker variables are dropped from its environment.
pub(crate) fn servers_for_cli(
    servers: &[McpServer],
    allowed: Option<&[String]>,
    workspace: &Path,
    provider: &str,
) -> Vec<McpServer> {
    let workspace = workspace.to_string_lossy();
    // The CLIs name a server's tools `mcp__<server>__<tool>` (Claude) and
    // `<server>_<tool>` (OpenCode), so a rule for one server must not match
    // another's tools.
    let ambiguous = |name: &str| {
        name.contains("__")
            || servers.iter().any(|other| {
                other.name != name
                    && (other.name.starts_with(&format!("{name}_"))
                        || name.starts_with(&format!("{}_", other.name)))
            })
    };
    servers
        .iter()
        .filter(|s| {
            // A command with a `/` that is not absolute would be found
            // relative to the workspace, which the model can write.
            let relative = s.command.contains('/') && !s.command.starts_with('/');
            let reason = if !is_plain_name(&s.name) {
                Some("its name may only use ASCII letters, digits, '_' and '-'")
            } else if ambiguous(&s.name) {
                Some("its name contains '__' or extends another server's name with '_'")
            } else if relative
                || !kernex_core::context::mcp_command_allowed(&s.allowed_commands, &s.command)
            {
                Some("its command is relative or not in a declared allow-list")
            } else if !allowlist_names_server(allowed, &s.name) {
                Some("the tool allowlist names none of its tools")
            } else if !s.network {
                Some(
                    "it declares no network access, and servers the CLI starts share the \
                     CLI's network",
                )
            } else {
                None
            };
            if let Some(reason) = reason {
                tracing::warn!("mcp: {:?} not passed to {provider}: {reason}", s.name);
            }
            reason.is_none()
        })
        .map(|s| {
            let (env, dropped) = kernex_core::spawn::filter_unsafe_env(&s.env);
            for key in &dropped {
                tracing::warn!(
                    server = %s.name,
                    env_key = %key,
                    "mcp: dropping dynamic-linker env var supplied by skill (would bypass sandbox)"
                );
            }
            McpServer {
                args: s
                    .args
                    .iter()
                    .map(|a| a.replace(WORKSPACE_PLACEHOLDER, &workspace))
                    .collect(),
                env,
                ..s.clone()
            }
        })
        .collect()
}

/// Ends the child's process group when dropped.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(group) = self.0 {
            kernex_sandbox::kill_process_group(group);
        }
    }
}

/// Run `cmd` to completion and collect its output, giving up after
/// `timeout`.
///
/// The child runs in its own process group, and the group is killed when
/// the call ends: on a timeout, when the caller stops waiting, and after a
/// normal exit, so the shell commands and servers the CLI started do not
/// outlive it. A process that left the group (`setsid`) is not reached.
/// `Err(Elapsed)` is the timeout; the inner result is the spawn or wait
/// outcome.
pub(crate) async fn run_with_timeout(
    mut cmd: tokio::process::Command,
    timeout: std::time::Duration,
) -> Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed> {
    use std::process::Stdio;
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Ok(Err(e)),
    };
    let group = child.id();
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    // Declared after `wait`, so it is dropped first: on a timeout or a
    // cancellation the group is killed while the child is still unreaped and
    // its id still names the group. After a normal exit the id names the
    // group for as long as any member is alive, and a free id is not handed
    // out again within the moment before the kill.
    let _guard = GroupGuard(group);
    tokio::time::timeout(timeout, &mut wait).await
}

/// ASCII letters, digits, `_` and `-`, at least one character.
pub(crate) fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Create `dir` (and its parents) and make it readable by the owner only.
///
/// `dir` itself must be a directory, not a symlink: the sandbox grants
/// write access to the place a granted path resolves to, so a link left
/// where a CLI keeps its state would carry the next call's access, and this
/// change of permissions, wherever it points.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[cfg(unix)]
    #[test]
    fn a_private_dir_that_is_a_symlink_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("state");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(create_private_dir(&link).is_err());
        let mode = std::fs::metadata(&victim).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);

        let plain = root.path().join("plain").join("dir");
        create_private_dir(&plain).unwrap();
        let mode = std::fs::metadata(&plain).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    fn server(name: &str, network: bool) -> McpServer {
        McpServer {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "pkg".into(), WORKSPACE_PLACEHOLDER.into()],
            env: HashMap::from([
                ("TOKEN".to_string(), "t".to_string()),
                ("DYLD_INSERT_LIBRARIES".to_string(), "/x.dylib".to_string()),
            ]),
            network,
            allowed_commands: vec!["npx".into()],
        }
    }

    #[test]
    fn a_temporary_workspace_is_removed_after_the_call() {
        let ws = CliWorkspace::for_call(None).unwrap();
        let path = ws.path().to_path_buf();
        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        drop(ws);
        assert!(!path.exists());
    }

    #[test]
    fn a_configured_workspace_is_used_as_given() {
        let dir = Path::new("/srv/project");
        let ws = CliWorkspace::for_call(Some(dir)).unwrap();
        assert_eq!(ws.path(), dir);
    }

    #[test]
    fn servers_are_prepared_for_the_cli() {
        let ws = Path::new("/srv/project");
        let out = servers_for_cli(&[server("fs", true)], None, ws, "test");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].args, vec!["-y", "pkg", "/srv/project"]);
        assert!(out[0].env.contains_key("TOKEN"));
        assert!(!out[0].env.contains_key("DYLD_INSERT_LIBRARIES"));
    }

    #[test]
    fn a_server_without_network_is_not_passed() {
        let out = servers_for_cli(&[server("fs", false)], None, Path::new("/w"), "test");
        assert!(out.is_empty());
    }

    #[test]
    fn a_server_outside_its_allow_list_is_not_passed() {
        let mut s = server("fs", true);
        s.allowed_commands = vec![];
        assert!(servers_for_cli(&[s], None, Path::new("/w"), "test").is_empty());
    }

    #[test]
    fn a_server_with_a_name_the_cli_would_split_is_not_passed() {
        for name in ["x,Bash,WebFetch,y", "a b", "fs(1)", "", "fs*"] {
            let out = servers_for_cli(&[server(name, true)], None, Path::new("/w"), "test");
            assert!(out.is_empty(), "{name:?}");
        }
        let out = servers_for_cli(&[server("my-fs_2", true)], None, Path::new("/w"), "test");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn servers_whose_tool_names_could_collide_are_not_passed() {
        let names = |servers: &[McpServer]| -> Vec<String> {
            servers_for_cli(servers, None, Path::new("/w"), "test")
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        assert!(names(&[server("fs", true), server("fs_x", true)]).is_empty());
        assert!(names(&[server("a__b", true)]).is_empty());
        assert_eq!(
            names(&[server("fs", true), server("fsx", true)]),
            vec!["fs", "fsx"]
        );
    }

    #[test]
    fn a_relative_command_is_not_passed() {
        for command in ["./npx", "node_modules/.bin/npx"] {
            let mut s = server("fs", true);
            s.command = command.into();
            assert!(servers_for_cli(&[s], None, Path::new("/w"), "test").is_empty());
        }
    }

    #[test]
    fn the_tool_allowlist_selects_servers() {
        let servers = [server("fs", true), server("db", true)];
        let names = |allowed: Option<&[String]>| -> Vec<String> {
            servers_for_cli(&servers, allowed, Path::new("/w"), "test")
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        assert_eq!(names(None), vec!["fs", "db"]);
        assert!(names(Some(&[])).is_empty());
        assert!(names(Some(&["Bash".to_string()])).is_empty());
        assert_eq!(names(Some(&["mcp__db".to_string()])), vec!["db"]);
        assert_eq!(names(Some(&["mcp__fs__read_file".to_string()])), vec!["fs"]);
        assert_eq!(names(Some(&["mcp__fs__*".to_string()])), vec!["fs"]);
    }
}
