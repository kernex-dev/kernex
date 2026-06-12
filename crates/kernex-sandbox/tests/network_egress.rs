#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the subprocess network egress policy.
//!
//! Runs a REAL sandboxed connect probe (curl against a local TCP listener)
//! and asserts that the default profile denies the connection while the
//! `allow_network` opt-in permits it. Skips when the host cannot apply
//! OS enforcement; on Linux the denial assertion additionally requires a
//! kernel with Landlock network rules (ABI v4, kernel 6.7+), since older
//! kernels cannot restrict the network (the documented best-effort gap).
//!
//! The probe targets 127.0.0.1 so no DNS resolution and no real egress is
//! involved; what is being proven is that the sandbox blocks the connect.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

fn run(cmd: tokio::process::Command) -> std::process::Output {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        let mut cmd = cmd;
        cmd.output().await.expect("spawn sandboxed probe")
    })
}

/// Minimal one-shot HTTP responder on 127.0.0.1; returns the bound port.
/// Accepts up to `max_conns` connections, answering each with a tiny 200.
fn spawn_local_http(max_conns: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local listener");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..max_conns {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    port
}

/// `curl` connect probe against the local listener, under the sandbox.
fn probe_cmd(data_dir: &Path, port: u16, allow_network: bool) -> tokio::process::Command {
    let profile = kernex_sandbox::SandboxProfile {
        allow_network,
        ..Default::default()
    };
    let mut cmd = kernex_sandbox::protected_command("curl", data_dir, &profile);
    cmd.arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg("10")
        .arg(format!("http://127.0.0.1:{port}/"));
    cmd
}

/// True when this host's sandbox can actually deny a TCP connect:
/// always on macOS Seatbelt; on Linux only with Landlock ABI v4+.
fn network_denial_enforceable() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/sys/kernel/security/landlock/abi_version")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|abi| abi >= 4)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[test]
fn network_egress_denied_by_default_allowed_on_opt_in() {
    if !kernex_sandbox::os_enforcement_available() {
        eprintln!("skipping network_egress: no OS-level sandbox enforcement on this host");
        return;
    }

    let ws = tempfile::tempdir().expect("temp workspace");
    let data_dir = ws.path().canonicalize().expect("canonicalize temp dir");
    let port = spawn_local_http(8);

    // 1. Opt-in first: proves the listener + curl harness works under the
    //    sandbox before anything is asserted about denial.
    let allowed = run(probe_cmd(&data_dir, port, true));
    let allowed_stdout = String::from_utf8_lossy(&allowed.stdout);
    assert!(
        allowed.status.success() && allowed_stdout.contains("ok"),
        "opt-in connect failed: status={:?} stdout={allowed_stdout} stderr={}",
        allowed.status,
        String::from_utf8_lossy(&allowed.stderr)
    );

    // 2. Default profile: the connect must be denied wherever the platform
    //    can enforce it.
    if network_denial_enforceable() {
        let denied = run(probe_cmd(&data_dir, port, false));
        let denied_stdout = String::from_utf8_lossy(&denied.stdout);
        assert!(
            !denied.status.success() && !denied_stdout.contains("ok"),
            "default-deny connect unexpectedly succeeded: status={:?} stdout={denied_stdout}",
            denied.status
        );
    } else {
        eprintln!(
            "skipping denial assertion: this kernel cannot restrict subprocess network egress"
        );
    }
}
