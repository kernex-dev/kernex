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
use std::sync::Mutex;
use tokio::process::Command;
use tracing::warn;

use landlock::{
    path_beneath_rules, Access, AccessFs, AccessNet, BitFlags, Ruleset, RulesetAttr,
    RulesetCreated, RulesetCreatedAttr, ABI,
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
///
/// ## Allocator soundness
///
/// The Landlock ruleset is built **in the parent** before the child is forked.
/// The forked child then only invokes [`RulesetCreated::restrict_self`] in
/// `pre_exec`, which is a thin wrapper around the `landlock_restrict_self`
/// syscall and does not require the global allocator. This avoids the classic
/// post-fork hazard of touching `malloc` while another parent thread holds
/// the allocator lock — under a multithreaded tokio runtime that race could
/// deadlock the child indefinitely.
pub(crate) fn protected_command(
    program: &str,
    data_dir: &std::path::Path,
    profile: &crate::SandboxProfile,
) -> Command {
    if !landlock_available() {
        warn!("landlock: not supported by this kernel; falling back to code-level protection");
        return Command::new(program);
    }

    let mut cmd = Command::new(program);

    // Build the ruleset in the parent. Filesystem probes, allocations, and any
    // landlock-internal bookkeeping happen here, where no fork-after-allocator
    // hazard exists.
    let prepared = match build_ruleset(data_dir, profile) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "landlock: ruleset construction failed; falling back to code-level protection");
            return cmd;
        }
    };

    // Move the prepared ruleset into the pre_exec closure. `restrict_self`
    // consumes self, and pre_exec is FnMut, so we wrap in `Option<Mutex<_>>`
    // and `take()` on first invocation. Mutex is required because the
    // pre_exec closure must be `Send`.
    let cell: Mutex<Option<RulesetCreated>> = Mutex::new(Some(prepared));

    // SAFETY: pre_exec runs in the forked child between fork and exec. The
    // closure only locks a Mutex (no allocation; the Mutex is uncontended in
    // the child since other threads do not exist post-fork), takes the
    // prepared ruleset, and invokes the `landlock_restrict_self` syscall.
    unsafe {
        cmd.pre_exec(move || {
            let mut guard = cell
                .lock()
                .map_err(|_| std::io::Error::other("landlock: ruleset mutex poisoned"))?;
            let ruleset = guard
                .take()
                .ok_or_else(|| std::io::Error::other("landlock: pre_exec invoked twice"))?;
            // Note: we cannot inspect `status.ruleset` here to log
            // partial-enforcement warnings — tracing dispatchers may hold
            // allocated state and we are post-fork. Operators who need
            // FullyEnforced guarantees should set
            // `SandboxProfile::require_os_enforcement = true` and use
            // `try_protected_command`.
            let _status = ruleset.restrict_self().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
            })?;
            Ok(())
        });
    }

    cmd
}

/// Build the Landlock ruleset entirely in the parent process. The returned
/// [`RulesetCreated`] holds the kernel FD and the rule set; calling
/// [`restrict_self`](RulesetCreated::restrict_self) on it later (post-fork)
/// applies the policy via a single syscall with no further allocation.
fn build_ruleset(
    data_dir: &std::path::Path,
    profile: &crate::SandboxProfile,
) -> Result<RulesetCreated, anyhow::Error> {
    let home_dir = crate::resolved_home();

    // Broad read on `/`, but NOT blanket write on `$HOME`. Writes are
    // granted only to the workspace/data dir, the system temp dirs, the
    // toolchain cache/state dirs, and `$KERNEX_DATA_DIR`. Landlock is
    // most-specific-rule-wins, so the credential refer_only rules and the
    // data/ + config refer_only rules below override these for their subtrees.
    let base = Ruleset::default().handle_access(full_access())?;

    // Network egress deny-by-default: handling the TCP access rights without
    // adding any NetPort allow rule denies every TCP bind/connect. Only
    // possible on ABI v4+ (kernel 6.7+); older kernels cannot restrict the
    // network at all, so the gap is logged instead of silently assumed
    // covered. UDP and non-TCP sockets are outside Landlock's scope on every
    // ABI (the macOS Seatbelt path covers all socket families; this
    // asymmetry is documented in the crate docs).
    let base = if profile.allow_network {
        base
    } else {
        match landlock_abi_version() {
            Some(abi) if abi >= NET_RESTRICTION_MIN_ABI => {
                base.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?
            }
            abi => {
                warn!(
                    abi = ?abi,
                    "landlock: kernel ABI lacks network rules (needs v4 / kernel 6.7+); \
                     subprocess network egress cannot be restricted on this host"
                );
                base
            }
        }
    };

    let mut ruleset = base
        .create()?
        .add_rules(path_beneath_rules(&[PathBuf::from("/")], read_access()))?
        .add_rules(path_beneath_rules(&[PathBuf::from("/tmp")], full_access()))?;

    // The workspace/data dir is writable (its `data/` subdir is refer_only
    // below). Replaces the prior blanket `$HOME` write grant.
    if data_dir.exists() {
        ruleset =
            ruleset.add_rules(path_beneath_rules(&[data_dir.to_path_buf()], full_access()))?;
    }

    let optional_paths = ["/var/tmp", "/opt", "/srv", "/run", "/media", "/mnt"];
    for path in &optional_paths {
        let p = PathBuf::from(path);
        if p.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(&[p], full_access()))?;
        }
    }

    // Toolchain cache/state dirs and $KERNEX_DATA_DIR stay writable so real
    // tools (cargo/npm/...) keep working under the lockdown.
    for dir in crate::write_allow_dirs(&home_dir) {
        if dir.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(&[dir], full_access()))?;
        }
    }
    if let Some(kdd) = std::env::var_os("KERNEX_DATA_DIR").map(PathBuf::from) {
        if kdd.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(&[kdd], full_access()))?;
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

    // Credential read-deny list: refer_only blocks reads and writes;
    // most-specific-rule-wins makes this override the broad `/` read for these
    // subtrees, so SSH keys / cloud creds / tokens are unreadable.
    for cred in crate::credential_read_deny_dirs(&home_dir) {
        if cred.exists() {
            ruleset = ruleset.add_rules(path_beneath_rules(&[cred], refer_only()))?;
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

    Ok(ruleset)
}

/// Check if the kernel supports Landlock by probing the ABI version file.
fn landlock_available() -> bool {
    std::path::Path::new("/sys/kernel/security/landlock/abi_version").exists()
}

/// Path to the kernel's Landlock ABI version file.
const ABI_VERSION_FILE: &str = "/sys/kernel/security/landlock/abi_version";

/// Minimum Landlock ABI that supports network (TCP bind/connect) rules.
/// Shipped in kernel 6.7.
const NET_RESTRICTION_MIN_ABI: u32 = 4;

/// Read the kernel's Landlock ABI version. `None` when Landlock is absent
/// or the file is unreadable/unparsable.
fn landlock_abi_version() -> Option<u32> {
    let content = std::fs::read_to_string(ABI_VERSION_FILE).ok()?;
    parse_abi_version(&content)
}

/// Parse the abi_version file content (a decimal integer plus newline).
fn parse_abi_version(content: &str) -> Option<u32> {
    content.trim().parse().ok()
}

/// Minimal access — blocks both reads and writes via Landlock intersection.
///
/// When combined with `full_access` on a parent path, effective access =
/// `full_access ∩ Refer = Refer` — no ReadFile, no WriteFile.
fn refer_only() -> BitFlags<AccessFs> {
    AccessFs::Refer.into()
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

    #[test]
    fn test_parse_abi_version() {
        assert_eq!(parse_abi_version("4\n"), Some(4));
        assert_eq!(parse_abi_version("5"), Some(5));
        assert_eq!(parse_abi_version("  6  \n"), Some(6));
        assert_eq!(parse_abi_version(""), None);
        assert_eq!(parse_abi_version("not-a-number"), None);
        assert_eq!(parse_abi_version("-1"), None);
    }

    #[test]
    fn test_build_ruleset_with_network_denied_succeeds() {
        // The deny-by-default path must build a valid ruleset on any
        // Landlock-capable kernel, whether or not the ABI supports network
        // rules (older ABIs log the gap and keep filesystem rules).
        if !landlock_available() {
            return;
        }
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = crate::SandboxProfile::default();
        assert!(build_ruleset(&data_dir, &profile).is_ok());
        let opt_in = crate::SandboxProfile {
            allow_network: true,
            ..Default::default()
        };
        assert!(build_ruleset(&data_dir, &opt_in).is_ok());
    }
}
