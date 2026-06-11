#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Reality gate for the D-13 (b) `$HOME` lockdown (Phase S batch S4).
//!
//! Runs REAL sandboxed subprocesses (Seatbelt on macOS, Landlock on Linux) and
//! asserts that a credential read is denied while a non-credential read and a
//! workspace write succeed. Skips when the host cannot apply OS enforcement
//! (e.g. Linux < 5.13, WSL1) - on CI the macOS and ubuntu runners both enforce.
//!
//! Everything is anchored to a throwaway `$HOME` (a `TempDir`) set for the
//! duration of the single test, so the credential rules apply to files this
//! test created rather than the runner's real home. One test fn => no
//! intra-binary parallelism touching `HOME`.

use std::path::Path;

fn run(cmd: tokio::process::Command) -> std::process::Output {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        let mut cmd = cmd;
        cmd.output().await.expect("spawn sandboxed probe")
    })
}

#[test]
fn home_lockdown_denies_credential_read_allows_workspace() {
    if !kernex_sandbox::os_enforcement_available() {
        eprintln!("skipping home_lockdown: no OS-level sandbox enforcement on this host");
        return;
    }

    let home = tempfile::tempdir().expect("temp home");
    // Canonicalize: on macOS the system temp dir is under /var (a symlink to
    // /private/var), and Seatbelt enforces on the canonical path. Real user
    // homes (/Users/name, /home/name) are not symlinked, so this only
    // normalizes the test fixture, not production behaviour.
    let home_path = home.path().canonicalize().expect("canonicalize temp home");

    // Populate the throwaway home: a credential file, a non-credential file,
    // and a workspace under the data dir.
    let ssh_dir = home_path.join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let ssh_key = ssh_dir.join("id_rsa");
    std::fs::write(&ssh_key, "PRIVATE-KEY-SENTINEL").unwrap();

    let readable = home_path.join("readable.txt");
    std::fs::write(&readable, "public").unwrap();

    let data_dir = home_path.join(".kernex");
    let workspace = data_dir.join("projects").join("proj");
    std::fs::create_dir_all(&workspace).unwrap();

    // SAFETY: single-test binary, HOME mutated once and restored at the end.
    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home_path);

    let ssh_out = run(cred_read_cmd(&data_dir, &ssh_key));
    let pub_out = run(cred_read_cmd(&data_dir, &readable));
    let write_target = workspace.join("agent-wrote.txt");
    let write_out = run(write_cmd(&data_dir, &write_target));

    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }

    // 1. Credential read is denied (exit-signal clause 2): either the process
    //    failed, or (defensively) the key sentinel never reached stdout.
    let ssh_stdout = String::from_utf8_lossy(&ssh_out.stdout);
    assert!(
        !ssh_out.status.success() || !ssh_stdout.contains("PRIVATE-KEY-SENTINEL"),
        "credential read was NOT denied: status={:?} stdout={ssh_stdout}",
        ssh_out.status
    );

    // 2. A non-credential read still works (reads stay broad).
    let pub_stdout = String::from_utf8_lossy(&pub_out.stdout);
    assert!(
        pub_out.status.success() && pub_stdout.contains("public"),
        "non-credential read was unexpectedly blocked: status={:?} stdout={pub_stdout}",
        pub_out.status
    );

    // 3. A workspace write succeeds (the agent can still do its job).
    assert!(
        write_out.status.success() && write_target.exists(),
        "workspace write was blocked: status={:?} stderr={}",
        write_out.status,
        String::from_utf8_lossy(&write_out.stderr)
    );

    // Note: the $HOME-dotfile write-denial is proven by the seatbelt/landlock
    // unit tests, not here: the test's throwaway home lives under the system
    // temp dir, which is write-allowed, so a dotfile-write probe would be a
    // platform-dependent confound.
}

/// `cat <path>` under the sandbox.
fn cred_read_cmd(data_dir: &Path, path: &Path) -> tokio::process::Command {
    let profile = kernex_sandbox::SandboxProfile::default();
    let mut cmd = kernex_sandbox::protected_command("/bin/cat", data_dir, &profile);
    cmd.arg(path);
    cmd
}

/// `sh -c 'printf x > <path>'` under the sandbox.
fn write_cmd(data_dir: &Path, path: &Path) -> tokio::process::Command {
    let profile = kernex_sandbox::SandboxProfile::default();
    let mut cmd = kernex_sandbox::protected_command("/bin/sh", data_dir, &profile);
    cmd.arg("-c")
        .arg(format!("printf x > {}", shell_quote(path)));
    cmd
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}
