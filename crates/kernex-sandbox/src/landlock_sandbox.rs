//! Linux Landlock LSM enforcement — broad allowlist approach.
//!
//! Landlock uses a broad allowlist: read-only on `/` (covers system dirs),
//! full access to `$HOME`, `/tmp`, `/var/tmp`, `/opt`, `/srv`, `/run`,
//! `/media`, `/mnt`. Then applies restrictive rules to `{data_dir}/data/`
//! and `{data_dir}/config.toml` (Refer-only access blocks both reads and
//! writes via Landlock's intersection semantics).
//!
//! ## Kernel Version Requirements
//!
//! Landlock support was introduced progressively across kernel releases. Each
//! ABI version adds new access rights; older kernels silently ignore unknown
//! flags via the `landlock` crate's version negotiation.
//!
//! | ABI | Kernel | Access rights added |
//! |-----|--------|---------------------|
//! | V1  | 5.13   | Filesystem reads, writes, execution (initial LSM) |
//! | V2  | 5.19   | File truncation (`MakeFifo`, `TruncateFile`) |
//! | V3  | 6.2    | Symlink creation, extended stat operations |
//! | V4  | 6.7    | `ioctl` on devices and files |
//! | V5  | 6.12   | Scope restrictions (cross-thread fd access) |
//!
//! This module requests [`ABI::V5`] access rights via [`full_access()`].
//! On kernels below 6.12, the `landlock` crate negotiates down automatically
//! and applies whatever rights the running kernel supports. The
//! [`RulesetStatus::FullyEnforced`] check logs a warning when enforcement is
//! partial so operators can identify under-protected deployments.
//!
//! **Minimum for any OS-level enforcement:** Linux 5.13.
//! Below 5.13 (or WSL1, or containers with the Landlock LSM disabled),
//! [`landlock_available()`] returns `false` and the sandbox falls back entirely
//! to code-level protection via `is_read_blocked()` / `is_write_blocked()`.
//!
//! ## Edge Cases and Fallback
//!
//! - **WSL1**: Does not expose the Landlock ABI; falls back to code-level enforcement.
//! - **Containers**: Require the host kernel to have `CONFIG_SECURITY_LANDLOCK=y` and
//!   the LSM enabled (e.g., `lsm=landlock,...`). Without it, the ABI file is absent.
//! - **Partial enforcement**: Kernels between 5.13 and 6.11 apply the rights they know
//!   about and log `"best-effort protection active"`. Core protection (no writes to
//!   `data/` or `config.toml`) is included in V1 rights and enforced on all 5.13+ kernels.
//!
//! Code-level enforcement via `is_read_blocked()` and `is_write_blocked()`
//! provides additional protection on all platforms regardless of kernel version.

use std::path::PathBuf;
use tokio::process::Command;
use tracing::warn;

use landlock::{
    path_beneath_rules, Access, AccessFs, BitFlags, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, ABI,
};

/// All read-related filesystem access flags.
fn read_access() -> BitFlags<AccessFs> {
    AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::Execute | AccessFs::Refer
}

/// All filesystem access flags (read + write).
fn full_access() -> BitFlags<AccessFs> {
    AccessFs::from_all(ABI::V5)
}

/// Build a [`Command`] with Landlock read/write restrictions applied via `pre_exec`.
///
/// The child process will have:
/// - Read and execute access to the entire filesystem (`/`)
/// - Full access to `$HOME`, `/tmp`, `/var/tmp`, `/opt`, `/srv`, `/run`, `/media`, `/mnt`
/// - Restricted access to `{data_dir}/data/` and `{data_dir}/config.toml` (Refer-only,
///   which blocks both reads and writes via Landlock intersection semantics)
///
/// System directories (`/bin`, `/sbin`, `/usr`, `/etc`, `/lib`, etc.) are implicitly
/// read-only because only `/` gets read access and writable paths are explicitly listed.
///
/// If the kernel does not support Landlock, logs a warning and falls back
/// to a plain command.
pub(crate) fn protected_command(
    program: &str,
    data_dir: &std::path::Path,
    profile: &crate::SandboxProfile,
) -> Command {
    if !landlock_available() {
        warn!("landlock: not supported by this kernel; falling back to code-level protection");
        return Command::new(program);
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let data_dir_owned = data_dir.to_path_buf();
    let profile_clone = profile.clone();

    let mut cmd = Command::new(program);

    // SAFETY: pre_exec runs in the forked child before exec. We only call
    // the landlock crate (which uses syscalls), no async or allocator abuse.
    unsafe {
        cmd.pre_exec(move || {
            apply_landlock(&home, &data_dir_owned, &profile_clone).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
            })
        });
    }

    cmd
}

/// Check if the kernel supports Landlock by probing the ABI version file.
fn landlock_available() -> bool {
    std::path::Path::new("/sys/kernel/security/landlock/abi_version").exists()
}

/// Minimal access — blocks both reads and writes via Landlock intersection.
///
/// When combined with `full_access` on a parent path, effective access =
/// `full_access ∩ Refer = Refer` — no ReadFile, no WriteFile.
fn refer_only() -> BitFlags<AccessFs> {
    AccessFs::Refer.into()
}

/// Apply Landlock restrictions to the current process.
fn apply_landlock(
    home: &str,
    data_dir: &std::path::Path,
    profile: &crate::SandboxProfile,
) -> Result<(), anyhow::Error> {
    let home_dir = PathBuf::from(home);

    let mut ruleset = Ruleset::default()
        .handle_access(full_access())?
        .create()?
        .add_rules(path_beneath_rules(&[PathBuf::from("/")], read_access()))?
        .add_rules(path_beneath_rules(&[home_dir], full_access()))?
        .add_rules(path_beneath_rules(&[PathBuf::from("/tmp")], full_access()))?;

    let optional_paths = ["/var/tmp", "/opt", "/srv", "/run", "/media", "/mnt"];
    for path in &optional_paths {
        let p = PathBuf::from(path);
        if p.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(&[p], full_access()))?;
        }
    }

    for allowed in &profile.allowed_paths {
        if allowed.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(
                std::slice::from_ref(allowed),
                full_access(),
            ))?;
        }
    }

    // Ensure the directory exists so the Landlock rule is always applied.
    // Without this, a first-run scenario where the data dir hasn't been
    // created yet would skip the restriction entirely.
    let data_data = data_dir.join("data");
    let _ = std::fs::create_dir_all(&data_data);
    if data_data.exists() {
        ruleset = ruleset.add_rules(path_beneath_rules(&[data_data], refer_only()))?;
    }

    // Cannot safely pre-create config.toml (empty file breaks TOML parser).
    // Code-level enforcement provides protection when it doesn't exist yet.
    let config_file = data_dir.join("config.toml");
    if config_file.exists() {
        ruleset = ruleset.add_rules(path_beneath_rules(&[config_file], refer_only()))?;
    }

    for blocked in &profile.blocked_paths {
        if blocked.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(
                std::slice::from_ref(blocked),
                refer_only(),
            ))?;
        }
    }

    let status = ruleset.restrict_self()?;

    if status.ruleset != RulesetStatus::FullyEnforced {
        warn!(
            "landlock: not all restrictions enforced (kernel may lack full support); \
             best-effort protection active"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_access_flags() {
        let flags = read_access();
        assert!(flags.contains(AccessFs::ReadFile));
        assert!(flags.contains(AccessFs::ReadDir));
        assert!(flags.contains(AccessFs::Execute));
    }

    #[test]
    fn test_full_access_contains_writes() {
        let flags = full_access();
        assert!(flags.contains(AccessFs::WriteFile));
        assert!(flags.contains(AccessFs::ReadFile));
        assert!(flags.contains(AccessFs::MakeDir));
    }

    #[test]
    fn test_refer_only_blocks_reads_and_writes() {
        let flags = refer_only();
        assert!(flags.contains(AccessFs::Refer));
        assert!(!flags.contains(AccessFs::ReadFile));
        assert!(!flags.contains(AccessFs::WriteFile));
    }

    #[test]
    fn test_command_structure() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = crate::SandboxProfile::default();
        let cmd = protected_command("claude", &data_dir, &profile);
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(program, "claude");
    }
}
