#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Probes for the macOS deny-default profile.
//!
//! Runs REAL programs under the generated Seatbelt profile, against the
//! runner's own home directory and toolchains: the profile is only useful if
//! ordinary development commands keep working inside it, and only safe if
//! the keychain and the directories of installed programs stay out of reach.
//!
//! A tool that is not installed is reported and skipped. `git`, `cargo`,
//! `python3` and `find` are expected on every macOS development host and
//! fail the test when missing.

use std::path::{Path, PathBuf};

use kernex_sandbox::{SandboxProfile, SandboxScope};

/// Environment variable that turns a skipped sandbox test into a failure.
const REQUIRE_ENV: &str = "KERNEX_TEST_REQUIRE_SANDBOX";

fn enforcement_or_skip(test: &str) -> bool {
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

/// The first `name` on `PATH`.
fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

struct Outcome {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `program args...` in `dir` under the sandbox.
fn sandboxed(
    scope: &SandboxScope,
    profile: &SandboxProfile,
    dir: &Path,
    program: &str,
    args: &[&str],
) -> Outcome {
    let mut cmd = kernex_sandbox::protected_command(program, scope, profile);
    cmd.args(args).current_dir(dir);
    // The environment is cleared down to a base list; a toolchain installed
    // outside its default location is found through these.
    for key in ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let out = rt.block_on(async move { cmd.output().await.expect("spawn sandboxed probe") });
    Outcome {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn workspace(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("workspace");
    let path = dir.path().canonicalize().expect("canonicalize workspace");
    (dir, path)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME"))
}

#[test]
fn toolchains_work_inside_the_profile() {
    if !enforcement_or_skip("toolchains") {
        return;
    }
    let (_guard, ws) = workspace("toolchains-");
    let scope = SandboxScope::new(&ws, home().join(".kernex"));
    let profile = SandboxProfile::default();
    let run = |program: &str, args: &[&str]| sandboxed(&scope, &profile, &ws, program, args);
    let mut skipped = Vec::new();

    // A fixture crate with no dependencies, outside any workspace.
    std::fs::create_dir_all(ws.join("fixture/src")).unwrap();
    std::fs::write(
        ws.join("fixture/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        ws.join("fixture/src/main.rs"),
        "fn main() { println!(\"built\"); }\n",
    )
    .unwrap();

    let shell = run(
        "/bin/sh",
        &[
            "-c",
            "echo one > a.txt && cat a.txt | wc -l && (echo sub) && kill -0 $$ && mktemp -t kx \
             && echo out > /dev/stdout && echo err > /dev/stderr && echo null > /dev/null",
        ],
    );
    assert!(shell.ok, "shell pipeline failed: {}", shell.stderr);

    assert!(tool("git").is_some(), "git is not installed");
    let git = run(
        "/bin/sh",
        &[
            "-c",
            "git init -q . && git add fixture \
             && git -c user.name=probe -c user.email=probe@example.invalid commit -q -m probe \
             && git status --short --branch && git log --oneline",
        ],
    );
    assert!(
        git.ok && git.stdout.contains("probe"),
        "git failed: {} {}",
        git.stdout,
        git.stderr
    );

    assert!(tool("cargo").is_some(), "cargo is not installed");
    let cargo = run(
        "cargo",
        &[
            "build",
            "--offline",
            "--manifest-path",
            "fixture/Cargo.toml",
        ],
    );
    assert!(cargo.ok, "cargo build failed: {}", cargo.stderr);
    let built = run("fixture/target/debug/fixture", &[]);
    assert!(
        built.ok && built.stdout.contains("built"),
        "the built program did not run: {}",
        built.stderr
    );

    assert!(tool("python3").is_some(), "python3 is not installed");
    let python = run(
        "python3",
        &[
            "-c",
            "import tempfile, pathlib; tempfile.NamedTemporaryFile().close(); \
             pathlib.Path('py.txt').write_text('py'); print('python ok')",
        ],
    );
    assert!(
        python.ok && python.stdout.contains("python ok"),
        "python3 failed: {}",
        python.stderr
    );

    let find = run("/usr/bin/find", &["fixture/src", "-name", "*.rs"]);
    assert!(
        find.ok && find.stdout.contains("main.rs"),
        "find failed: {}",
        find.stderr
    );

    if tool("node").is_some() {
        let node = run(
            "node",
            &[
                "-e",
                "require('fs').writeFileSync('node.txt', 'x'); console.log('node ok')",
            ],
        );
        assert!(
            node.ok && node.stdout.contains("node ok"),
            "node failed: {}",
            node.stderr
        );
    } else {
        skipped.push("node");
    }

    if tool("rg").is_some() {
        let rg = run("rg", &["-l", "built", "fixture/src"]);
        assert!(
            rg.ok && rg.stdout.contains("main.rs"),
            "rg failed: {}",
            rg.stderr
        );
    } else {
        skipped.push("rg");
    }

    eprintln!("toolchains: not installed, skipped: {skipped:?}");
}

#[test]
fn installed_programs_and_system_dirs_are_not_writable() {
    if !enforcement_or_skip("write_limits") {
        return;
    }
    let (_guard, ws) = workspace("writes-");
    let scope = SandboxScope::new(&ws, home().join(".kernex"));
    let profile = SandboxProfile::default();
    let name = format!("kx-probe-{}", std::process::id());

    // Directories of installed programs, a system-wide prefix and the home
    // directory itself. A directory that does not exist is refused too: the
    // probe would have to create it.
    let targets = [
        home().join(".cargo/bin").join(&name),
        home().join(".rustup").join(&name),
        home().join(".local/bin").join(&name),
        PathBuf::from("/usr/local").join(&name),
        PathBuf::from("/opt/homebrew").join(&name),
        home().join(&name),
    ];
    for target in &targets {
        let script = format!("printf x > '{}'", target.display());
        let out = sandboxed(&scope, &profile, &ws, "/bin/sh", &["-c", &script]);
        let created = target.exists();
        if created {
            let _ = std::fs::remove_file(target);
        }
        assert!(
            !out.ok && !created,
            "write to {} was not denied",
            target.display()
        );
    }
}

#[test]
fn local_sockets_stay_closed_when_the_network_is_allowed() {
    if !enforcement_or_skip("local_sockets") {
        return;
    }
    assert!(tool("python3").is_some(), "python3 is not installed");
    // A short path: a Unix socket address is limited to about 100 bytes.
    let dir = tempfile::Builder::new()
        .prefix("kx-")
        .tempdir_in("/tmp")
        .expect("socket dir");
    let ws = dir.path().canonicalize().expect("canonicalize");
    let socket = ws.join("daemon.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let scope = SandboxScope::new(&ws, home().join(".kernex"));
    let connect = format!(
        "import socket; s = socket.socket(socket.AF_UNIX); s.connect('{}'); print('connected')",
        socket.display()
    );

    // The probe itself works: the same connect succeeds outside the sandbox.
    let baseline = std::process::Command::new("python3")
        .args(["-c", &connect])
        .output()
        .expect("run python3");
    assert!(baseline.status.success(), "baseline connect failed");

    for profile in [
        SandboxProfile::default(),
        SandboxProfile::new().with_allow_network(true),
    ] {
        let out = sandboxed(&scope, &profile, &ws, "python3", &["-c", &connect]);
        assert!(
            !out.ok && !out.stdout.contains("connected"),
            "a local socket was reachable (allow_network={})",
            profile.allow_network
        );
    }
}

#[test]
fn keychain_is_out_of_reach_unless_allowed() {
    if !enforcement_or_skip("keychain") {
        return;
    }
    let (_guard, ws) = workspace("keychain-");
    let scope = SandboxScope::new(&ws, home().join(".kernex"));
    let query = |profile: &SandboxProfile| {
        sandboxed(
            &scope,
            profile,
            &ws,
            "/usr/bin/security",
            &["list-keychains"],
        )
    };

    // The query must work outside the sandbox, or its failure inside proves
    // nothing.
    let baseline = std::process::Command::new("/usr/bin/security")
        .arg("list-keychains")
        .output()
        .expect("run security");
    if !baseline.status.success() {
        eprintln!("skipping keychain: the query fails outside the sandbox on this host");
        return;
    }

    let denied = query(&SandboxProfile::default());
    assert!(!denied.ok, "keychain query succeeded: {}", denied.stdout);

    // Network access must not bring the keychain with it.
    let with_network = query(&SandboxProfile::new().with_allow_network(true));
    assert!(
        !with_network.ok,
        "keychain query succeeded with the network allowed: {}",
        with_network.stdout
    );

    // A search for an item that does not exist. When the keychain cannot be
    // reached the search itself cannot be created, which `security` reports
    // by naming the call that failed.
    const SEARCH_REFUSED: &str = "SecKeychainSearchCreateFromAttributes";
    let search = |profile: &SandboxProfile| {
        sandboxed(
            &scope,
            profile,
            &ws,
            "/usr/bin/security",
            &["find-generic-password", "-s", "kx-probe-no-such-item"],
        )
    };
    for profile in [
        SandboxProfile::default(),
        SandboxProfile::new().with_allow_network(true),
    ] {
        let out = search(&profile);
        assert!(
            !out.ok && out.stderr.contains(SEARCH_REFUSED),
            "keychain search ran inside the sandbox: {}",
            out.stderr
        );
    }
    let ran = search(&SandboxProfile::new().with_allow_keychain(true));
    assert!(
        !ran.stderr.contains(SEARCH_REFUSED) && ran.stderr.contains("could not be found"),
        "keychain search did not run with the keychain allowed: {}",
        ran.stderr
    );

    let allowed = query(&SandboxProfile::new().with_allow_keychain(true));
    assert!(
        allowed.ok,
        "keychain query failed with the keychain allowed: {}",
        allowed.stderr
    );
}
