//! Environment hardening for spawned subprocesses.

use tokio::process::Command;

/// Environment variables preserved when hardening a spawned subprocess.
///
/// Everything else from the parent environment is cleared - in particular
/// provider API keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) and cloud
/// credentials, which subprocesses must never inherit implicitly. A
/// subprocess that legitimately needs more receives it via an explicit
/// `Command::env` opt-in at its call site (configured toolbox/MCP env maps,
/// the Claude CLI auth pass-through).
///
/// The `LC_*` locale family is additionally preserved by prefix in
/// [`hardened_env`]. Proxy and TLS-trust variables are preserved because they
/// are configuration, not secrets, and clearing them would silently break
/// corporate networks.
pub const BASE_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TERM",
    "USER",
    "LOGNAME",
    "SHELL",
    "TZ",
    "LANG",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// Clear the inherited environment on `cmd` and re-apply only the minimal
/// base allowlist ([`BASE_ENV_ALLOWLIST`] plus the `LC_*` family) from the
/// parent process.
///
/// Applied automatically by [`protected_command`](crate::protected_command) /
/// [`try_protected_command`](crate::try_protected_command);
/// exposed for spawn sites that cannot route through the sandbox wrappers
/// (e.g. MCP server processes that must stay unwrapped for stdio framing).
pub fn hardened_env(cmd: &mut Command) {
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        let Some(key) = k.to_str() else { continue };
        let allowed = BASE_ENV_ALLOWLIST.contains(&key) || key.starts_with("LC_");
        if allowed {
            cmd.env(&k, &v);
        }
    }
}
