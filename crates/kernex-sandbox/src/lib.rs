#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! # kernex-sandbox
//!
//! OS-level system protection for AI agent subprocesses.
//!
//! Uses a **blocklist** approach: everything is allowed by default, then
//! dangerous system directories and the runtime's core data are blocked.
//!
//! - **macOS**: Apple Seatbelt via `sandbox-exec -p <profile>` — denies reads
//!   and writes to `{data_dir}/data/` (memory.db) and `config.toml`; denies
//!   writes to `/System`, `/bin`, `/sbin`, `/usr/{bin,sbin,lib,libexec}`,
//!   `/private/etc`, `/Library`.
//! - **Linux**: Landlock LSM via `pre_exec` hook (kernel 5.13+ minimum, 6.12+
//!   for full ABI::V5 enforcement; older kernels apply best-effort protection
//!   using only the rights they support) — broad read-only on `/` with full
//!   access to `$HOME`, `/tmp`, `/var/tmp`, `/opt`, `/srv`, `/run`, `/media`,
//!   `/mnt`; restricted access to `{data_dir}/data/` and `config.toml`.
//! - **Other**: Falls back to a plain command with a warning.
//!
//! [`protected_command`] is best-effort and never fails: when the host
//! cannot apply OS-level enforcement it returns an unsandboxed command
//! and logs a warning. For deployments where running unsandboxed is
//! unacceptable, set `SandboxProfile::require_os_enforcement = true` and
//! call [`try_protected_command`] instead, which surfaces an
//! [`std::io::Error`] when enforcement is unavailable. [`os_enforcement_available`]
//! reports the host's capability without building a command.
//!
//! Also provides [`is_write_blocked`] and [`is_read_blocked`] for code-level
//! enforcement in tool executors (protects memory.db and config.toml on all
//! platforms).
//!
//! This crate is intentionally standalone with zero internal dependencies,
//! making it usable outside the Kernex ecosystem.

use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Configuration for system sandbox restrictions.
#[derive(Clone, Debug, Default)]
pub struct SandboxProfile {
    /// Extra paths that should be fully writable (Linux Landlock allowlist).
    pub allowed_paths: Vec<PathBuf>,
    /// Extra paths that should be completely blocked for read/write.
    pub blocked_paths: Vec<PathBuf>,
    /// When `true`, [`try_protected_command`] returns an error if OS-level
    /// enforcement is unavailable (no Seatbelt on macOS, no Landlock kernel
    /// support on Linux, or an unsupported platform). Defaults to `false`,
    /// which preserves the historical fail-open behaviour of
    /// [`protected_command`]: a warning is logged and a plain command is
    /// returned. Set this to `true` for security-sensitive deployments where
    /// running unsandboxed is unacceptable.
    pub require_os_enforcement: bool,
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use tracing::warn;

#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "linux")]
mod landlock_sandbox;

/// Build a [`Command`] with OS-level system protection (best-effort).
///
/// Blocks writes to dangerous system directories and the runtime's core
/// data directory (memory.db). On platforms without OS-level enforcement
/// (no `/usr/bin/sandbox-exec`, no Landlock kernel support, Windows, etc.)
/// this falls back to a plain command with a warning, preserving the
/// historical fail-open behaviour. For deployments where running
/// unsandboxed is unacceptable, set `profile.require_os_enforcement = true`
/// and use [`try_protected_command`].
///
/// `data_dir` is the runtime data directory (e.g. `~/.kernex/`). Writes to
/// `{data_dir}/data/` are blocked (protects memory.db). All other paths
/// under `data_dir` (workspace, skills, projects) remain writable.
pub fn protected_command(program: &str, data_dir: &Path, profile: &SandboxProfile) -> Command {
    let mut cmd = platform_command(program, data_dir, profile);
    hardened_env(&mut cmd);
    cmd
}

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
/// Applied automatically by [`protected_command`] / [`try_protected_command`];
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

/// Credential directories/files under `$HOME` whose READS are denied to
/// sandboxed subprocesses (D-13 option (b) credential read-deny list).
///
/// Reads stay broad everywhere else; only these high-value secret stores are
/// blocked, so an agent that is prompted (or tricked) into exfiltrating SSH
/// keys, cloud credentials, or auth tokens is stopped at the OS layer. The
/// list is relative-suffix joined onto the resolved `home`.
pub fn credential_read_deny_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".ssh",
        ".aws",
        ".gnupg",
        ".kube",
        ".docker",
        ".netrc",
        ".config/gh",
        ".config/gcloud",
        ".config/google-chrome",
        ".mozilla",
        // macOS browser/credential stores
        "Library/Application Support/Google/Chrome",
        "Library/Application Support/Firefox",
        "Library/Keychains",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Directories whose WRITES are allowed under the D-13 option (b) posture
/// (writes are otherwise denied inside `$HOME`).
///
/// Covers the per-user state/cache dirs that real toolchains write to during
/// normal operation (cargo, rustup, npm/yarn/pnpm, deno, bun, the XDG and
/// macOS cache trees). Writes to the workspace/data dir, `$KERNEX_DATA_DIR`,
/// and the system temp dirs are allowed separately by the platform modules.
///
/// **(a)-fallback tuning point:** if a real toolchain writes somewhere outside
/// this list and breaks, the fix is to extend this list (or, per D-13, fall
/// back to a credential-deny-list-only posture). CI cannot exercise every
/// toolchain, so this is the most likely place to need a follow-up.
pub fn write_allow_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".cache",
        ".cargo",
        ".rustup",
        ".npm",
        ".yarn",
        ".pnpm-store",
        ".deno",
        ".bun",
        ".local/state",
        ".local/share",
        ".gradle",
        ".m2",
        ".gem",
        "Library/Caches",
        "Library/Developer",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Resolve `$HOME`, falling back to `/tmp` (matches the Landlock builder's
/// prior behaviour for headless/no-HOME environments).
pub(crate) fn resolved_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Strict variant of [`protected_command`].
///
/// Returns an [`io::Error`](std::io::Error) of kind [`Unsupported`](std::io::ErrorKind::Unsupported)
/// when the host cannot apply OS-level enforcement *and*
/// `profile.require_os_enforcement` is `true`. When the flag is `false`
/// this behaves identically to [`protected_command`] and always returns
/// `Ok(Command)`.
pub fn try_protected_command(
    program: &str,
    data_dir: &Path,
    profile: &SandboxProfile,
) -> std::io::Result<Command> {
    if profile.require_os_enforcement && !os_enforcement_available() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "OS-level sandbox enforcement is required but unavailable on this host",
        ));
    }
    let mut cmd = platform_command(program, data_dir, profile);
    hardened_env(&mut cmd);
    Ok(cmd)
}

/// Returns `true` when the current host can apply OS-level sandbox
/// enforcement (Seatbelt on macOS or Landlock on Linux). Used by
/// [`try_protected_command`] to honour `require_os_enforcement`.
pub fn os_enforcement_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(target_os = "linux")]
    {
        Path::new("/sys/kernel/security/landlock/abi_version").exists()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Best-effort path canonicalization. Returns the canonicalized path or the
/// original if canonicalization fails (file doesn't exist yet, permissions, etc.).
///
/// **Assumption (callers must uphold):** the input path is already absolute,
/// either because the caller passed an `is_absolute()` path through or because
/// the upstream code used `data_dir.join(...)` on an absolute `data_dir`. The
/// lexical fallback is safe under that assumption: if canonicalization fails
/// because the leaf does not yet exist, the parent components are still
/// resolved and the comparison vs. blocked prefixes (`starts_with`) remains
/// meaningful. Passing a relative path here would silently pass through and
/// could match an unintended prefix. The two callers (`is_blocked_write_path`
/// and `is_allowed_data_dir_write`) both prefix-promote relative inputs via
/// `cwd.join(...)` before reaching this function.
fn try_canonicalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Check if a write to the given path should be blocked.
///
/// Returns `true` if the path targets a protected location:
/// - Dangerous OS directories (`/System`, `/bin`, `/sbin`, `/usr/bin`, etc.)
/// - The runtime's core data directory (`{data_dir}/data/`) — protects memory.db
///
/// Resolves symlinks before comparison to prevent bypass via symlink chains.
/// Used by tool executors for code-level enforcement.
pub fn is_write_blocked(path: &Path, data_dir: &Path, profile: Option<&SandboxProfile>) -> bool {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        return true;
    };

    let resolved = try_canonicalize(&abs);

    let data_data = try_canonicalize(&data_dir.join("data"));
    if resolved.starts_with(&data_data) {
        return true;
    }

    let config_file = try_canonicalize(&data_dir.join("config.toml"));
    if resolved == config_file {
        return true;
    }

    if let Some(prof) = profile {
        for blocked in &prof.blocked_paths {
            if resolved.starts_with(try_canonicalize(blocked)) {
                return true;
            }
        }
    }

    let blocked_prefixes: &[&str] = &[
        "/System",
        "/bin",
        "/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/libexec",
        "/private/etc",
        "/Library",
        "/etc",
        "/boot",
        "/proc",
        "/sys",
        "/dev",
    ];

    for prefix in blocked_prefixes {
        if resolved.starts_with(prefix) {
            return true;
        }
    }

    false
}

/// Check if a read from the given path should be blocked.
///
/// Returns `true` if the path targets a protected location:
/// - The runtime's core data directory (`{data_dir}/data/`) — protects memory.db
/// - The runtime's config file (`{data_dir}/config.toml`) — protects API keys
/// - The actual config file at `config_path` (may differ from data_dir) — protects secrets
///
/// Resolves symlinks before comparison to prevent bypass via symlink chains.
/// Used by tool executors for code-level enforcement.
pub fn is_read_blocked(
    path: &Path,
    data_dir: &Path,
    config_path: Option<&Path>,
    profile: Option<&SandboxProfile>,
) -> bool {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        return true;
    };

    let resolved = try_canonicalize(&abs);

    let data_data = try_canonicalize(&data_dir.join("data"));
    if resolved.starts_with(&data_data) {
        return true;
    }

    let config_in_data = try_canonicalize(&data_dir.join("config.toml"));
    if resolved == config_in_data {
        return true;
    }

    if let Some(cp) = config_path {
        let resolved_config = try_canonicalize(cp);
        if resolved == resolved_config {
            return true;
        }
    }

    if let Some(prof) = profile {
        for blocked in &prof.blocked_paths {
            if resolved.starts_with(try_canonicalize(blocked)) {
                return true;
            }
        }
    }

    false
}

#[cfg(target_os = "macos")]
fn platform_command(program: &str, data_dir: &Path, profile: &SandboxProfile) -> Command {
    seatbelt::protected_command(program, data_dir, profile)
}

#[cfg(target_os = "linux")]
fn platform_command(program: &str, data_dir: &Path, profile: &SandboxProfile) -> Command {
    landlock_sandbox::protected_command(program, data_dir, profile)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_command(program: &str, _data_dir: &Path, _profile: &SandboxProfile) -> Command {
    warn!("OS-level protection not available on this platform; using code-level enforcement only");
    Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_protected_command_returns_command() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = SandboxProfile::default();
        let cmd = protected_command("claude", &data_dir, &profile);
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert!(!program.is_empty());
    }

    #[test]
    fn test_protected_command_hardens_env() {
        // A parent var outside the allowlist must not survive; the base
        // allowlist (PATH) and the LC_* family must.
        std::env::set_var("KERNEX_TEST_SENTINEL_DO_NOT_PASS", "1");
        std::env::set_var("LC_KERNEX_TEST", "es_ES.UTF-8");

        let data_dir = PathBuf::from("/tmp/ws");
        let profile = SandboxProfile::default();
        let cmd = protected_command("echo", &data_dir, &profile);
        let explicit: Vec<String> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| v.map(|_| k.to_string_lossy().to_string()))
            .collect();

        std::env::remove_var("KERNEX_TEST_SENTINEL_DO_NOT_PASS");
        std::env::remove_var("LC_KERNEX_TEST");

        assert!(
            !explicit
                .iter()
                .any(|k| k == "KERNEX_TEST_SENTINEL_DO_NOT_PASS"),
            "non-allowlisted parent var re-applied after env_clear"
        );
        assert!(
            explicit.iter().any(|k| k == "LC_KERNEX_TEST"),
            "LC_* family not preserved"
        );
        assert!(
            explicit.iter().any(|k| k == "PATH"),
            "PATH not preserved from the base allowlist"
        );
    }

    #[test]
    fn test_is_write_blocked_data_dir() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_write_blocked(
            Path::new("/home/user/.kernex/data/memory.db"),
            &data_dir,
            None
        ));
        assert!(is_write_blocked(
            Path::new("/home/user/.kernex/data/"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_allows_workspace() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(!is_write_blocked(
            Path::new("/home/user/.kernex/workspace/test.txt"),
            &data_dir,
            None
        ));
        assert!(!is_write_blocked(
            Path::new("/home/user/.kernex/skills/test/SKILL.md"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_system_dirs() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_write_blocked(
            Path::new("/System/Library/test"),
            &data_dir,
            None
        ));
        assert!(is_write_blocked(Path::new("/bin/sh"), &data_dir, None));
        assert!(is_write_blocked(Path::new("/usr/bin/env"), &data_dir, None));
        assert!(is_write_blocked(
            Path::new("/private/etc/hosts"),
            &data_dir,
            None
        ));
        assert!(is_write_blocked(
            Path::new("/Library/Preferences/test"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_allows_normal_paths() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(!is_write_blocked(Path::new("/tmp/test"), &data_dir, None));
        assert!(!is_write_blocked(
            Path::new("/home/user/documents/test"),
            &data_dir,
            None
        ));
        assert!(!is_write_blocked(
            Path::new("/usr/local/bin/something"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_no_string_prefix_false_positive() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(!is_write_blocked(
            Path::new("/binaries/test"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_relative_path() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_write_blocked(
            Path::new("relative/path"),
            &data_dir,
            None
        ));
        assert!(is_write_blocked(
            Path::new("../../data/memory.db"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_write_blocked_config_toml() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_write_blocked(
            Path::new("/home/user/.kernex/config.toml"),
            &data_dir,
            None
        ));
    }

    #[test]
    fn test_is_read_blocked_data_dir() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_read_blocked(
            Path::new("/home/user/.kernex/data/memory.db"),
            &data_dir,
            None,
            None
        ));
        assert!(is_read_blocked(
            Path::new("/home/user/.kernex/data/"),
            &data_dir,
            None,
            None
        ));
    }

    #[test]
    fn test_is_read_blocked_config() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_read_blocked(
            Path::new("/home/user/.kernex/config.toml"),
            &data_dir,
            None,
            None
        ));
    }

    #[test]
    fn test_is_read_blocked_external_config() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let ext_config = PathBuf::from("/opt/kernex/config.toml");
        assert!(is_read_blocked(
            Path::new("/opt/kernex/config.toml"),
            &data_dir,
            Some(ext_config.as_path()),
            None
        ));
        assert!(!is_read_blocked(
            Path::new("/opt/kernex/other.toml"),
            &data_dir,
            Some(ext_config.as_path()),
            None
        ));
    }

    #[test]
    fn test_is_read_blocked_allows_workspace() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(!is_read_blocked(
            Path::new("/home/user/.kernex/workspace/test.txt"),
            &data_dir,
            None,
            None
        ));
        assert!(!is_read_blocked(
            Path::new("/home/user/.kernex/skills/test/SKILL.md"),
            &data_dir,
            None,
            None
        ));
    }

    #[test]
    fn test_is_read_blocked_allows_stores() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(!is_read_blocked(
            Path::new("/home/user/.kernex/stores/trading/store.db"),
            &data_dir,
            None,
            None
        ));
    }

    #[test]
    fn test_try_protected_command_lenient_returns_ok() {
        // require_os_enforcement = false → always Ok, even when the platform
        // cannot enforce.
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = SandboxProfile::default();
        let result = try_protected_command("claude", &data_dir, &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_protected_command_strict_matches_capability() {
        // require_os_enforcement = true → result depends on host capability.
        // On a host that supports enforcement we expect Ok; on one that does
        // not we expect Err(ErrorKind::Unsupported). This guards against the
        // strict flag silently being ignored.
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = SandboxProfile {
            require_os_enforcement: true,
            ..Default::default()
        };
        let result = try_protected_command("claude", &data_dir, &profile);
        if os_enforcement_available() {
            assert!(result.is_ok(), "expected Ok on a host with enforcement");
        } else {
            let err = result.expect_err("expected Err on a host without enforcement");
            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        }
    }

    #[test]
    fn test_is_read_blocked_relative_path() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        assert!(is_read_blocked(
            Path::new("relative/path"),
            &data_dir,
            None,
            None
        ));
        assert!(is_read_blocked(
            Path::new("../../data/memory.db"),
            &data_dir,
            None,
            None
        ));
    }
}
