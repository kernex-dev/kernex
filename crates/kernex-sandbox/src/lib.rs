//! # kernex-sandbox
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
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
//! - **Linux**: Landlock LSM via `pre_exec` hook (kernel 5.13+) — broad
//!   read-only on `/` with full access to `$HOME`, `/tmp`, `/var/tmp`, `/opt`,
//!   `/srv`, `/run`, `/media`, `/mnt`; restricted access to `{data_dir}/data/`
//!   and `config.toml`.
//! - **Other**: Falls back to a plain command with a warning.
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
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use tracing::warn;

#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "linux")]
mod landlock_sandbox;

/// Build a [`Command`] with OS-level system protection.
///
/// Always active — blocks writes to dangerous system directories and
/// the runtime's core data directory (memory.db). No configuration needed.
///
/// `data_dir` is the runtime data directory (e.g. `~/.kernex/`). Writes to
/// `{data_dir}/data/` are blocked (protects memory.db). All other paths
/// under `data_dir` (workspace, skills, projects) remain writable.
///
/// On unsupported platforms, logs a warning and returns a plain command.
pub fn protected_command(program: &str, data_dir: &Path, profile: &SandboxProfile) -> Command {
    platform_command(program, data_dir, profile)
}

/// Best-effort path canonicalization. Returns the canonicalized path or the
/// original if canonicalization fails (file doesn't exist yet, permissions, etc.).
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
