//! Linux Landlock LSM enforcement, generated from the shared [`Policy`].
//!
//! Landlock only grants: a rule on a path adds rights for everything below
//! it, and no rule can take a right away. A path is therefore denied by
//! leaving it without a rule, never by a more specific rule.
//!
//! - **Reads.** Read access is not granted on `/`. Starting at `/`, every
//!   directory entry that does not lead to a read-deny path gets a read rule,
//!   and only the entries that do lead to one are descended into. For a deny
//!   list under `$HOME` that enumerates `/`, `/home`, the home directory and
//!   a level or two below it.
//! - **Writes.** Each write root gets full access. A root that contains a
//!   protected path is walked the same way, so the protected path is left
//!   out instead of being overridden.
//! - **Devices.** A short list of device files (`/dev/null`, `/dev/tty`, ...)
//!   and the shared-memory directory `/dev/shm` stay writable, so
//!   redirections and POSIX semaphores keep working. `ioctl` on a device
//!   file opened inside the sandbox is denied, and pseudo-terminals cannot
//!   be allocated.
//!
//! ## Limits of this design
//!
//! - A directory that was walked gets the right to be listed and nothing
//!   else. A Landlock rule covers everything below the directory it names,
//!   so the protected directories below it can be listed too: their file
//!   names are visible, their contents are not. A file created directly
//!   inside a walked directory after the ruleset was
//!   built is not readable or writable by that subprocess. The ruleset is
//!   rebuilt on every spawn, so this affects a file created and reopened
//!   within one command. With the default policy the walked directories are
//!   `/`, the home directory and its ancestors, and the runtime data
//!   directory. A `blocked_paths` entry inside the workspace makes the
//!   workspace root one of them: files can then only be created in its
//!   subdirectories.
//! - Symbolic links get no rule of their own. Access through a link is
//!   decided by the rules covering the file it points to.
//! - Landlock does not restrict connecting to a pathname Unix socket, so a
//!   subprocess can still talk to any daemon whose socket file it can reach:
//!   a container runtime socket, the session bus, an SSH or GPG agent, a
//!   terminal multiplexer. UDP and other non-TCP sockets are never
//!   restricted. Do not run the sandbox on a host where such a socket grants
//!   more than the sandbox is meant to allow.
//! - Landlock does not restrict `chmod`, `chown`, timestamps or extended
//!   attributes, nor `stat`. A subprocess can change the mode of a file it
//!   owns anywhere, and can see that a protected path exists.
//! - Rules follow the file, not its name. A hard link or a bind mount made
//!   before the sandbox started, pointing at a protected file from a
//!   readable place, exposes that file. The sandbox cannot create either.
//!
//! ## Kernel versions
//!
//! | ABI | Kernel | Added |
//! |-----|--------|-------|
//! | 1   | 5.13   | Filesystem read, write, execute, create and remove |
//! | 2   | 5.19   | Rename and link across directories (`Refer`) |
//! | 3   | 6.2    | File truncation |
//! | 4   | 6.7    | TCP bind and connect |
//! | 5   | 6.10   | `ioctl` on device files |
//! | 6   | 6.12   | Scoping of abstract Unix sockets and signals |
//!
//! The ruleset asks for everything up to ABI 6 and the `landlock` crate
//! drops what the running kernel does not know. Below ABI 2 a sandboxed
//! process cannot rename or link across directories at all. Below ABI 4 the
//! network cannot be restricted; the gap is logged at spawn time.
//!
//! Below ABI 3 a file outside the write roots can still be truncated, so
//! such a kernel does not count as enforcing: the rules it knows are
//! applied, [`enforces`] reports `false`, and a caller that requires
//! enforcement is refused.
//!
//! The ABI is read with the `landlock_create_ruleset` syscall. A kernel
//! without Landlock (older than 5.13, WSL1, or the LSM not enabled) reports
//! no ABI, and the sandbox falls back to the code-level checks unless
//! enforcement is required.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tokio::process::Command;
use tracing::warn;

use landlock::{
    path_beneath_rules, Access, AccessFs, AccessNet, BitFlags, Ruleset, RulesetAttr,
    RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope, ABI,
};

use crate::policy::Policy;

/// `LANDLOCK_CREATE_RULESET_VERSION`: ask the syscall for the ABI version.
const CREATE_RULESET_VERSION: libc::c_uint = 1;

/// Minimum Landlock ABI that counts as enforcing the policy. Earlier ABIs
/// cannot stop a file outside the write roots from being truncated.
const MIN_ENFORCING_ABI: u32 = 3;

/// Minimum Landlock ABI that supports network (TCP bind/connect) rules.
const NET_RESTRICTION_MIN_ABI: u32 = 4;

/// Device files a subprocess may read and write. Without them `> /dev/null`
/// and terminal output fail under the sandbox.
const DEVICE_FILES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

/// Shared-memory directory. POSIX semaphores and shared memory live here.
const SHARED_MEMORY_DIR: &str = "/dev/shm";

static WALKED_ROOT_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);
static PARTIAL_ABI_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Read access for a file or a directory tree.
fn read_access() -> BitFlags<AccessFs> {
    AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::Execute
}

/// All filesystem access flags (read + write).
fn full_access() -> BitFlags<AccessFs> {
    AccessFs::from_all(ABI::V5)
}

/// Access for the device files in [`DEVICE_FILES`]. No `ioctl`: on a
/// terminal that would allow injecting input into the user's session.
fn device_access() -> BitFlags<AccessFs> {
    AccessFs::ReadFile | AccessFs::WriteFile
}

/// The Landlock ABI of the running kernel, or `None` when Landlock is not
/// available. Probed once per process.
pub(crate) fn abi_version() -> Option<u32> {
    static ABI_VERSION: OnceLock<Option<u32>> = OnceLock::new();
    *ABI_VERSION.get_or_init(probe_abi)
}

/// Whether the running kernel enforces the whole filesystem policy.
pub(crate) fn enforces() -> bool {
    abi_version().is_some_and(|abi| abi >= MIN_ENFORCING_ABI)
}

fn probe_abi() -> Option<u32> {
    // SAFETY: `landlock_create_ruleset` with a null attribute pointer, size 0
    // and the VERSION flag reads no memory and creates nothing; it returns
    // the ABI version, or -1 with errno set when Landlock is absent.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    u32::try_from(version).ok().filter(|abi| *abi >= 1)
}

/// Build a [`Command`] with the Landlock ruleset for `policy` applied via
/// `pre_exec`.
///
/// If the kernel does not support Landlock, logs a warning and falls back
/// to a plain command. If the ruleset cannot be built, an error is returned
/// when enforcement is required and the command falls back otherwise.
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
    policy: &Policy,
    profile: &crate::SandboxProfile,
) -> std::io::Result<Command> {
    let Some(abi) = abi_version() else {
        warn!("landlock: not supported by this kernel; falling back to code-level protection");
        return Ok(Command::new(program));
    };
    if abi < MIN_ENFORCING_ABI && !PARTIAL_ABI_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        warn!(
            abi,
            "landlock: this kernel cannot restrict file truncation (needs ABI 3 / kernel 6.2); \
             applying the rules it supports"
        );
    }
    let required = crate::enforcement_required(profile);

    // Build the ruleset in the parent. Filesystem probes, allocations, and any
    // landlock-internal bookkeeping happen here, where no fork-after-allocator
    // hazard exists.
    let prepared = match build_ruleset(policy, profile) {
        Ok(r) => r,
        Err(e) if required => {
            return Err(std::io::Error::other(format!(
                "landlock ruleset could not be built: {e}"
            )));
        }
        Err(e) => {
            warn!(error = %e, "landlock: ruleset construction failed; falling back to code-level protection");
            return Ok(Command::new(program));
        }
    };

    let mut cmd = Command::new(program);

    // Move the prepared ruleset into the pre_exec closure. `restrict_self`
    // consumes self, and pre_exec is FnMut, so we wrap in `Option<Mutex<_>>`
    // and `take()` on first invocation. Mutex is required because the
    // pre_exec closure must be `Send`.
    let cell: Mutex<Option<RulesetCreated>> = Mutex::new(Some(prepared));

    // SAFETY: pre_exec runs in the forked child between fork and exec. The
    // closure only locks a Mutex (no allocation; the Mutex is uncontended in
    // the child since other threads do not exist post-fork), takes the
    // prepared ruleset, and invokes the `landlock_restrict_self` syscall.
    // Errors are built from a raw errno, which does not allocate.
    unsafe {
        cmd.pre_exec(move || {
            let denied = || std::io::Error::from_raw_os_error(libc::EPERM);
            let mut guard = cell.lock().map_err(|_| denied())?;
            let ruleset = guard.take().ok_or_else(denied)?;
            let status = ruleset.restrict_self().map_err(|_| denied())?;
            // A ruleset the kernel did not enforce at all must not pass for
            // a sandbox when the caller required one.
            if required && status.ruleset == RulesetStatus::NotEnforced {
                return Err(denied());
            }
            Ok(())
        });
    }

    Ok(cmd)
}

/// Build the Landlock ruleset entirely in the parent process. The returned
/// [`RulesetCreated`] holds the kernel FD and the rule set; calling
/// [`restrict_self`](RulesetCreated::restrict_self) on it later (post-fork)
/// applies the policy via a single syscall with no further allocation.
fn build_ruleset(
    policy: &Policy,
    profile: &crate::SandboxProfile,
) -> Result<RulesetCreated, anyhow::Error> {
    let mut base = Ruleset::default().handle_access(full_access())?;

    // Network egress deny-by-default: handling the TCP access rights without
    // adding any NetPort allow rule denies every TCP bind/connect. Only
    // possible on ABI v4+ (kernel 6.7+); older kernels cannot restrict the
    // network at all, so the gap is logged instead of silently assumed
    // covered.
    if !profile.allow_network {
        match abi_version() {
            Some(abi) if abi >= NET_RESTRICTION_MIN_ABI => {
                base = base.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?;
            }
            abi => warn!(
                abi = ?abi,
                "landlock: kernel ABI lacks network rules (needs v4 / kernel 6.7+); \
                 subprocess network egress cannot be restricted on this host"
            ),
        }
    }
    // On ABI v6+ a sandboxed process may only signal processes inside its
    // own sandbox and only connect to abstract Unix sockets created there.
    // That is local IPC, independent of the network flag.
    base = base.scope(Scope::Signal | Scope::AbstractUnixSocket)?;

    let rules = PathRules::for_policy(policy);
    if let Some(root) = rules.walked_root(policy) {
        if !WALKED_ROOT_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
            warn!(
                root = %root.display(),
                "landlock: a protected path sits inside this writable directory; \
                 files can only be created in its subdirectories"
            );
        }
    }

    let ruleset = base
        .create()?
        // Listing only. Without it `ls` fails in `/`, in the home directory
        // and in a workspace that holds a protected path.
        .add_rules(path_beneath_rules(&rules.walked, AccessFs::ReadDir))?
        .add_rules(path_beneath_rules(&rules.read, read_access()))?
        .add_rules(path_beneath_rules(&rules.write, full_access()))?
        .add_rules(path_beneath_rules(DEVICE_FILES, device_access()))?
        .add_rules(path_beneath_rules([SHARED_MEMORY_DIR], full_access()))?;
    Ok(ruleset)
}

/// The paths that receive a rule, and the directories that were walked
/// instead of receiving one.
#[derive(Debug, Default)]
struct PathRules {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    walked: Vec<PathBuf>,
}

impl PathRules {
    fn for_policy(policy: &Policy) -> Self {
        let mut rules = Self::default();
        complement(
            Path::new("/"),
            policy.read_deny(),
            &mut rules.read,
            &mut rules.walked,
        );

        // A path that may not be read may not be written either.
        let mut write_deny = policy.write_deny().to_vec();
        write_deny.extend(policy.read_deny().iter().cloned());
        for root in policy.write_roots() {
            complement(root, &write_deny, &mut rules.write, &mut rules.walked);
        }
        rules
    }

    /// The first write root that had to be walked, if any.
    fn walked_root<'a>(&self, policy: &'a Policy) -> Option<&'a PathBuf> {
        policy
            .write_roots()
            .iter()
            .find(|root| self.walked.contains(root))
    }
}

/// Collect into `granted` the largest set of paths under `start` that
/// contains no `deny` path, descending only into directories that lead to
/// one. Directories that are descended into are recorded in `walked`; they
/// get the right to be listed and no other.
fn complement(
    start: &Path,
    deny: &[PathBuf],
    granted: &mut Vec<PathBuf>,
    walked: &mut Vec<PathBuf>,
) {
    if deny.iter().any(|denied| start.starts_with(denied)) {
        return;
    }
    if !deny.iter().any(|denied| denied.starts_with(start)) {
        granted.push(start.to_path_buf());
        return;
    }
    walked.push(start.to_path_buf());
    let entries = match std::fs::read_dir(start) {
        Ok(entries) => entries,
        Err(e) => {
            // Nothing below is granted. Say so: the subprocess will see
            // the whole directory as inaccessible.
            warn!(dir = %start.display(), error = %e, "landlock: cannot list directory; nothing below it is granted");
            return;
        }
    };
    for entry in entries.flatten() {
        // A rule on a symlink would apply to its target, which may be a
        // denied path. The target is covered where it really lives.
        if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            continue;
        }
        complement(&entry.path(), deny, granted, walked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::HostDirs;
    use crate::{SandboxProfile, SandboxScope};

    #[test]
    fn test_read_access_has_no_write_or_link_right() {
        let flags = read_access();
        assert!(flags.contains(AccessFs::ReadFile));
        assert!(flags.contains(AccessFs::ReadDir));
        assert!(flags.contains(AccessFs::Execute));
        assert!(!flags.contains(AccessFs::WriteFile));
        // Refer on a read-only tree would let a file be hard-linked into a
        // write root and modified there.
        assert!(!flags.contains(AccessFs::Refer));
    }

    #[test]
    fn test_device_access_has_no_ioctl() {
        let flags = device_access();
        assert!(flags.contains(AccessFs::WriteFile));
        assert!(!flags.contains(AccessFs::IoctlDev));
    }

    #[test]
    fn test_full_access_contains_writes() {
        let flags = full_access();
        assert!(flags.contains(AccessFs::WriteFile));
        assert!(flags.contains(AccessFs::ReadFile));
        assert!(flags.contains(AccessFs::MakeDir));
    }

    /// A tree: root/{a/{keep,secret/key},b,link->a/secret}.
    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/secret")).unwrap();
        std::fs::write(root.join("a/secret/key"), "k").unwrap();
        std::fs::write(root.join("a/keep"), "x").unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::os::unix::fs::symlink(root.join("a/secret"), root.join("link")).unwrap();
        (dir, root)
    }

    fn walk(start: &Path, deny: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let (mut granted, mut walked) = (Vec::new(), Vec::new());
        complement(start, deny, &mut granted, &mut walked);
        granted.sort();
        walked.sort();
        (granted, walked)
    }

    #[test]
    fn test_complement_grants_the_whole_tree_without_a_deny_path() {
        let (_dir, root) = tree();
        let (granted, walked) = walk(&root, &[PathBuf::from("/elsewhere")]);
        assert_eq!(granted, vec![root]);
        assert!(walked.is_empty());
    }

    #[test]
    fn test_complement_leaves_out_the_deny_path_and_its_ancestors() {
        let (_dir, root) = tree();
        let (granted, walked) = walk(&root, &[root.join("a/secret")]);
        assert_eq!(granted, vec![root.join("a/keep"), root.join("b")]);
        assert_eq!(walked, vec![root.clone(), root.join("a")]);
    }

    #[test]
    fn test_complement_skips_symlinks() {
        let (_dir, root) = tree();
        let (granted, _) = walk(&root, &[root.join("a/secret")]);
        assert!(!granted.contains(&root.join("link")));
    }

    #[test]
    fn test_complement_grants_nothing_under_a_denied_start() {
        let (_dir, root) = tree();
        let (granted, walked) = walk(&root.join("a/secret"), &[root.join("a")]);
        assert!(granted.is_empty());
        assert!(walked.is_empty());
    }

    #[test]
    fn test_complement_handles_a_deny_path_that_does_not_exist() {
        let (_dir, root) = tree();
        let (granted, walked) = walk(&root, &[root.join("b/missing.db")]);
        assert_eq!(granted, vec![root.join("a")]);
        assert_eq!(walked, vec![root.clone(), root.join("b")]);
    }

    #[test]
    fn test_policy_rules_keep_protected_state_out_of_both_lists() {
        let (_dir, root) = tree();
        let host = HostDirs {
            home: root.join("home"),
            temp_dirs: Vec::new(),
        };
        // State inside the workspace: `a` is the workspace, `a/secret` a
        // blocked path.
        let scope = SandboxScope::new(root.join("a"), root.join("state"));
        let profile = SandboxProfile::new().with_blocked_paths(vec![root.join("a/secret")]);
        let policy = Policy::with_host(&scope, &profile, &host);
        let rules = PathRules::for_policy(&policy);

        let secret = root.join("a/secret");
        assert!(rules.write.contains(&root.join("a/keep")));
        for granted in rules.read.iter().chain(&rules.write) {
            assert!(!granted.starts_with(&secret), "{}", granted.display());
            assert!(!secret.starts_with(granted), "{}", granted.display());
        }
        assert_eq!(rules.walked_root(&policy), Some(&root.join("a")));
    }

    #[test]
    fn test_build_ruleset_succeeds_with_and_without_network() {
        if abi_version().is_none() {
            return;
        }
        let scope = SandboxScope::new("/tmp/ws", "/tmp/state");
        let profile = SandboxProfile::default();
        assert!(build_ruleset(&Policy::new(&scope, &profile), &profile).is_ok());
        let opt_in = SandboxProfile::new().with_allow_network(true);
        assert!(build_ruleset(&Policy::new(&scope, &opt_in), &opt_in).is_ok());
    }

    #[test]
    fn test_command_structure() {
        let scope = SandboxScope::new("/tmp/ws", "/tmp/state");
        let profile = SandboxProfile::default();
        let cmd = protected_command("claude", &Policy::new(&scope, &profile), &profile).unwrap();
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(program, "claude");
    }
}
