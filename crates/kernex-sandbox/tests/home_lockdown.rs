#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the `$HOME` lockdown posture.
//!
//! Runs REAL sandboxed subprocesses (Seatbelt on macOS, Landlock on Linux) and
//! asserts what a tool call can and cannot reach: the workspace is writable,
//! its siblings and the runtime data directory are not, credential stores and
//! the memory database are unreadable, and reads stay broad otherwise. Skips
//! when the host cannot apply OS enforcement (e.g. Linux < 5.13, WSL1), unless
//! `KERNEX_TEST_REQUIRE_SANDBOX` is set, in which case it fails.
//!
//! Everything is anchored to a throwaway `$HOME` set for the duration of the
//! single test, so the rules apply to files this test created rather than the
//! runner's real home. The throwaway home lives under the cargo target
//! directory, not under the system temp directory: temp directories are
//! writable by policy and would hide every write denial. One test fn => no
//! intra-binary parallelism touching `HOME`.

use std::path::Path;

use kernex_sandbox::{SandboxProfile, SandboxScope};

/// Environment variable that turns a skipped sandbox test into a failure.
/// CI sets it on every runner that is known to enforce.
const REQUIRE_ENV: &str = "KERNEX_TEST_REQUIRE_SANDBOX";

/// Whether the probes can run. Panics instead of skipping when the
/// environment says this host must enforce.
fn enforcement_or_skip(test: &str) -> bool {
    eprintln!(
        "{test}: os_enforcement_available={} landlock_abi={:?}",
        kernex_sandbox::os_enforcement_available(),
        kernex_sandbox::landlock_abi_version()
    );
    if kernex_sandbox::os_enforcement_available() {
        return true;
    }
    let required = std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty() && v != "0");
    assert!(
        !required,
        "{test}: {REQUIRE_ENV} is set but this host has no OS-level sandbox enforcement"
    );
    eprintln!("skipping {test}: no OS-level sandbox enforcement on this host");
    false
}

fn run(cmd: tokio::process::Command) -> std::process::Output {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        let mut cmd = cmd;
        cmd.output().await.expect("spawn sandboxed probe")
    })
}

/// `cat <path>` under the sandbox; returns stdout when the read succeeded.
fn read_probe(scope: &SandboxScope, path: &Path) -> Option<String> {
    let mut cmd = kernex_sandbox::protected_command("/bin/cat", scope, &SandboxProfile::default());
    cmd.arg(path);
    let out = run(cmd);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `sh -c 'printf probe > <path>'` under the sandbox; returns whether the
/// file ended up holding the probe text.
fn write_probe(scope: &SandboxScope, path: &Path) -> bool {
    let mut cmd = kernex_sandbox::protected_command("/bin/sh", scope, &SandboxProfile::default());
    cmd.arg("-c")
        .arg(format!("printf probe > {}", shell_quote(path)));
    let out = run(cmd);
    let wrote = std::fs::read_to_string(path).is_ok_and(|s| s == "probe");
    assert_eq!(
        out.status.success(),
        wrote,
        "exit status and file content disagree for {}: stderr={}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    wrote
}

/// `sh -c <script>` under the sandbox; returns success and stdout.
fn shell_probe(scope: &SandboxScope, profile: &SandboxProfile, script: &str) -> (bool, String) {
    let mut cmd = kernex_sandbox::protected_command("/bin/sh", scope, profile);
    cmd.arg("-c").arg(script);
    let out = run(cmd);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

#[test]
fn home_lockdown_confines_writes_and_denies_secret_reads() {
    if !enforcement_or_skip("home_lockdown") {
        return;
    }

    let home = tempfile::Builder::new()
        .prefix("home-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("throwaway home");
    // Canonicalize: the OS enforces on the resolved path.
    let home_path = home.path().canonicalize().expect("canonicalize home");

    // Populate the throwaway home.
    let ssh_key = home_path.join(".ssh/id_rsa");
    std::fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
    std::fs::write(&ssh_key, "PRIVATE-KEY-SENTINEL").unwrap();

    let readable = home_path.join("readable.txt");
    std::fs::write(&readable, "public").unwrap();

    let data_dir = home_path.join(".kernex");
    let skill = data_dir.join("skills/review/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "skill body").unwrap();
    let database = data_dir.join("memory.db");
    std::fs::write(&database, "DATABASE-SENTINEL").unwrap();

    // The workspace name carries the characters a profile generator must not
    // trip over.
    let workspace = home_path.join("code/proj (copy) \"v2\"");
    let sibling = home_path.join("code/other");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();

    // A blocked directory inside the workspace.
    let secrets = workspace.join("secrets");
    let token = secrets.join("token");
    std::fs::create_dir_all(&secrets).unwrap();
    // An existing subdirectory: on Linux a file cannot be created directly
    // in a directory that holds a blocked path, only below it.
    let subdir = workspace.join("src");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(&token, "TOKEN-SENTINEL").unwrap();
    let blocking = SandboxProfile::new().with_blocked_paths(vec![secrets.clone()]);

    let scope = SandboxScope::new(&workspace, &data_dir);

    // SAFETY: single-test binary, HOME mutated once and restored at the end.
    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home_path);

    let ssh_read = read_probe(&scope, &ssh_key);
    let public_read = read_probe(&scope, &readable);
    let skill_read = read_probe(&scope, &skill);
    let database_read = read_probe(&scope, &database);

    let workspace_write = write_probe(&scope, &workspace.join("agent-wrote.txt"));
    let sibling_write = write_probe(&scope, &sibling.join("agent-wrote.txt"));
    let dotfile_write = write_probe(&scope, &home_path.join(".zshrc"));
    let skill_write = write_probe(&scope, &skill);
    let new_skill_write = write_probe(&scope, &data_dir.join("skills/review/extra.md"));
    let data_dir_write = write_probe(&scope, &data_dir.join("notes.txt"));
    let database_write = write_probe(&scope, &database);

    let dev_null = shell_probe(&scope, &SandboxProfile::default(), "echo x > /dev/null");
    let token_read = shell_probe(&scope, &blocking, &format!("cat {}", shell_quote(&token)));
    let moved_dir = workspace.join("moved");
    let move_blocked_dir = shell_probe(
        &scope,
        &blocking,
        &format!(
            "mv {} {} && cat {}",
            shell_quote(&secrets),
            shell_quote(&moved_dir),
            shell_quote(&moved_dir.join("token"))
        ),
    );
    let moved_workspace = home_path.join("code/proj-moved");
    let move_workspace = shell_probe(
        &scope,
        &blocking,
        &format!(
            "mv {} {}",
            shell_quote(&workspace),
            shell_quote(&moved_workspace)
        ),
    );
    // Ordinary work inside the workspace: list, create, read back, remove.
    let roundtrip = shell_probe(
        &scope,
        &SandboxProfile::default(),
        &format!(
            "cd {} && ls . > /dev/null && mkdir d && echo body > d/f && cat d/f && mv d e && rm -r e",
            shell_quote(&workspace)
        ),
    );
    // A hard link must not turn a read-only file into a writable one.
    let hard_link = workspace.join("linked.txt");
    let link_escape = shell_probe(
        &scope,
        &SandboxProfile::default(),
        &format!(
            "ln {} {} && printf hacked > {}",
            shell_quote(&readable),
            shell_quote(&hard_link),
            shell_quote(&hard_link)
        ),
    );
    let readable_after = std::fs::read_to_string(&readable).unwrap();
    let write_beside_blocked = shell_probe(
        &scope,
        &blocking,
        &format!("printf ok > {}", shell_quote(&subdir.join("beside.txt"))),
    );

    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }

    // Reads: credential stores and the memory database are denied; everything
    // else, including the skills directory, stays readable.
    assert_eq!(ssh_read, None, "credential read was not denied");
    assert_eq!(database_read, None, "memory database read was not denied");
    assert_eq!(public_read.as_deref(), Some("public"));
    assert_eq!(skill_read.as_deref(), Some("skill body"));

    // Writes: only the workspace.
    assert!(workspace_write, "workspace write was blocked");
    assert!(
        !sibling_write,
        "write to a sibling of the workspace succeeded"
    );
    assert!(!dotfile_write, "write to a $HOME dotfile succeeded");
    assert!(!skill_write, "overwrite of an installed skill succeeded");
    assert!(
        !new_skill_write,
        "write under the skills directory succeeded"
    );
    assert!(!data_dir_write, "write into the data directory succeeded");
    assert!(!database_write, "write to the memory database succeeded");
    assert_eq!(
        std::fs::read_to_string(&database).unwrap(),
        "DATABASE-SENTINEL"
    );

    // Device files stay usable.
    assert!(dev_null.0, "write to /dev/null was blocked");

    // A blocked directory inside the workspace is unreadable, and cannot be
    // reached by moving it or the workspace above it.
    assert!(!token_read.1.contains("TOKEN-SENTINEL"), "blocked read");
    assert!(
        !move_blocked_dir.1.contains("TOKEN-SENTINEL"),
        "blocked directory was moved and read"
    );
    assert!(
        !move_workspace.0,
        "workspace above a blocked path was moved"
    );
    assert!(token.exists() && !moved_dir.exists() && !moved_workspace.exists());
    assert!(write_beside_blocked.0, "write beside a blocked path failed");

    assert!(
        roundtrip.0 && roundtrip.1.contains("body"),
        "ordinary work inside the workspace failed: {roundtrip:?}"
    );
    assert!(
        !link_escape.0 && readable_after == "public",
        "a file outside the workspace was modified through a hard link"
    );
}
