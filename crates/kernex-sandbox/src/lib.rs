#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! # kernex-sandbox
//!
//! OS-level system protection for AI agent subprocesses.
//!
//! Every check starts from a [`SandboxScope`]: the workspace a tool call
//! works in and the runtime data directory it must not change. One path
//! policy is derived from the scope and the [`SandboxProfile`], and both the
//! OS profile and the code-level checks read it:
//!
//! - **Writes** are allowed under the workspace, the system temp
//!   directories, the per-user toolchain cache directories
//!   ([`write_allow_dirs`], minus their `bin` directories) and
//!   `SandboxProfile::allowed_paths`. The data
//!   directory is not a write root, and its memory database, `skills/`,
//!   `projects/` and configuration file stay denied even when the data
//!   directory sits inside the workspace.
//! - **Reads** stay broad, minus the credential stores
//!   ([`credential_read_deny_dirs`]), the memory database, the configuration
//!   file and `SandboxProfile::blocked_paths`.
//! - A workspace that is the home directory, one of its ancestors or `/` is
//!   not accepted as a write root. [`try_protected_command`] refuses to spawn
//!   when enforcement is required; otherwise writes fall back to the temp
//!   directories and a warning is printed once.
//!
//! Platform back ends:
//!
//! - **macOS**: Apple Seatbelt via `sandbox-exec -p <profile>`. The profile
//!   denies by default: a subprocess may run programs, read files outside
//!   the read-deny paths, write under the write roots and reach a short
//!   list of system services. It cannot query the keychain, talk to other
//!   applications or allocate a pseudo-terminal.
//! - **Linux**: Landlock LSM via `pre_exec` hook (kernel 5.13+ minimum;
//!   older kernels than 6.12 apply the rights they support). Reads are
//!   granted everywhere except the read-deny paths, and writes only under
//!   the write roots.
//! - **Other**: Falls back to a plain command with a warning.
//!
//! [`protected_command`] is best-effort and never fails: when the host
//! cannot apply OS-level enforcement it returns an unsandboxed command
//! after emitting a once-per-process stderr warning. For deployments where
//! running unsandboxed is unacceptable, set
//! `SandboxProfile::require_os_enforcement = true` (or export
//! `KERNEX_REQUIRE_SANDBOX=1`, which applies process-wide without code
//! changes) and call [`try_protected_command`] instead, which surfaces an
//! [`std::io::Error`] when enforcement is unavailable. [`os_enforcement_available`]
//! reports the host's capability without building a command.
//!
//! The fail-open fallback is the 0.x default; kernex 1.0 flips the library
//! default to fail-closed (`require_os_enforcement = true`). Embedders that
//! need today's lenient behaviour at 1.0 will have to opt out explicitly.
//!
//! ## Platform enforcement notes
//!
//! - **macOS**: enforcement relies on `/usr/bin/sandbox-exec`, which Apple
//!   documents as deprecated but still ships and uses in its own tooling.
//!   There is no supported third-party replacement API. If a future macOS
//!   release removes it, [`os_enforcement_available`] returns `false` and
//!   the fail-mode policy above applies.
//! - **Linux**: Landlock enforcement depth depends on the kernel ABI
//!   (5.13 minimum; 5.19 adds cross-directory rename and link control; 6.7
//!   adds TCP restrictions; 6.12 adds scoping of abstract Unix sockets and
//!   signals). An older kernel enforces the rights it knows.
//!   [`os_enforcement_available`] reports Landlock presence and
//!   [`landlock_abi_version`] the depth. Landlock denies a path by giving it
//!   no rule, so files cannot be created directly inside a directory on the
//!   way to a protected path; with the default policy those are `/`, the
//!   home directory and its ancestors, and the data directory. Such a
//!   directory can be listed, and so can the protected directories below
//!   it: their file names are visible on Linux, their contents are not.
//!
//! ## Subprocess network policy
//!
//! Sandboxed subprocesses are denied network access by default
//! (`SandboxProfile::allow_network = false`); tools that genuinely need
//! egress opt in explicitly. Coverage is asymmetric by platform:
//!
//! - **macOS Seatbelt**: full. Every socket operation is denied (TCP, UDP,
//!   and local sockets; DNS resolution is consequently unavailable too).
//!   A subprocess that is allowed the network also gets the system's DNS
//!   and certificate-trust services.
//! - **Linux Landlock**: TCP bind/connect on kernels 6.7+ (ABI v4) and
//!   abstract Unix sockets on 6.12+ (ABI v6). Older kernels cannot restrict
//!   the network; the gap is logged at spawn time. UDP, other non-TCP
//!   sockets and pathname Unix sockets are never restricted, so a daemon
//!   whose socket file the subprocess can reach stays reachable.
//!
//! In-process HTTP performed by the runtime itself (e.g. a `web_fetch`
//! tool implemented without a subprocess) is not subject to this policy;
//! it has its own request-level validation.
//!
//! Also provides [`is_write_blocked`] and [`is_read_blocked`], which apply
//! the subprocess policy to a path on every platform, and the narrower
//! checks for in-process file tools ([`tool_read_path`],
//! [`tool_write_path`]): those confine a tool to the workspace,
//! `allowed_paths` and `read_paths`, because no OS profile stands behind
//! code that runs inside the host process.
//!
//! This crate is intentionally standalone with zero internal dependencies,
//! making it usable outside the Kernex ecosystem.

use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Configuration for system sandbox restrictions.
///
/// Build one with [`SandboxProfile::new`] and the `with_*` methods; the
/// struct is `#[non_exhaustive]`, so a later field is not a breaking change.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SandboxProfile {
    /// Extra paths that should be fully writable.
    pub allowed_paths: Vec<PathBuf>,
    /// Extra paths that should be completely blocked for read/write.
    pub blocked_paths: Vec<PathBuf>,
    /// Extra directories the in-process file tools may read, beside the
    /// workspace and `allowed_paths` (see [`tool_read_path`]). Subprocess
    /// reads are not affected: they are broad already.
    pub read_paths: Vec<PathBuf>,
    /// When `true`, [`try_protected_command`] returns an error if OS-level
    /// enforcement is unavailable (no Seatbelt on macOS, no Landlock kernel
    /// support on Linux, or an unsupported platform). Defaults to `false`,
    /// which preserves the historical fail-open behaviour of
    /// [`protected_command`]: a once-per-process stderr warning is emitted
    /// and a plain command is returned. Set this to `true` for
    /// security-sensitive deployments where running unsandboxed is
    /// unacceptable, or export [`REQUIRE_SANDBOX_ENV`] to get the same
    /// effect process-wide without touching code.
    ///
    /// The default flips to `true` in kernex 1.0.
    pub require_os_enforcement: bool,
    /// When `true`, the sandboxed subprocess may open network connections.
    /// Defaults to `false`: subprocess network egress is denied by default
    /// so a compromised or prompt-injected tool cannot exfiltrate data.
    ///
    /// Enforcement is platform-asymmetric and documented in the crate docs:
    /// macOS Seatbelt denies all socket access (TCP, UDP, and local
    /// sockets, which also disables DNS); Linux Landlock denies TCP
    /// bind/connect on kernels 6.7+ (ABI v4) and cannot restrict the
    /// network at all on older kernels (logged at spawn time). UDP and
    /// non-TCP sockets are never restricted on Linux.
    pub allow_network: bool,
    /// When `true`, the sandboxed subprocess may query the macOS keychain.
    /// Defaults to `false`. For a host that runs an agent CLI as its
    /// provider and that CLI keeps its login in the keychain. No effect
    /// on other platforms.
    pub allow_keychain: bool,
}

impl SandboxProfile {
    /// The default profile: no extra paths, enforcement not required,
    /// subprocess network denied.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the extra writable paths.
    pub fn with_allowed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.allowed_paths = paths;
        self
    }

    /// Set the paths blocked for both reads and writes.
    pub fn with_blocked_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.blocked_paths = paths;
        self
    }

    /// Set the extra directories the file tools may read.
    pub fn with_read_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.read_paths = paths;
        self
    }

    /// Require OS-level enforcement (see
    /// [`SandboxProfile::require_os_enforcement`]).
    pub fn with_require_os_enforcement(mut self, required: bool) -> Self {
        self.require_os_enforcement = required;
        self
    }

    /// Allow or deny subprocess network access (see
    /// [`SandboxProfile::allow_network`]).
    pub fn with_allow_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    /// Allow or deny keychain queries on macOS (see
    /// [`SandboxProfile::allow_keychain`]).
    pub fn with_allow_keychain(mut self, allow: bool) -> Self {
        self.allow_keychain = allow;
        self
    }
}

/// Environment variable that requires OS-level sandbox enforcement
/// process-wide, equivalent to setting
/// [`SandboxProfile::require_os_enforcement`] on every profile.
///
/// Any value other than empty, `0`, or `false` (case-insensitive) counts as
/// set. Honoured by [`try_protected_command`] and by every caller that
/// routes through [`enforcement_required`]. [`protected_command`] is
/// infallible by signature and cannot honour it; strict deployments must use
/// [`try_protected_command`].
pub const REQUIRE_SANDBOX_ENV: &str = "KERNEX_REQUIRE_SANDBOX";

/// Returns `true` when OS-level sandbox enforcement is required, either by
/// the profile flag or by the [`REQUIRE_SANDBOX_ENV`] environment variable.
pub fn enforcement_required(profile: &SandboxProfile) -> bool {
    profile.require_os_enforcement || env_requires_sandbox()
}

fn env_requires_sandbox() -> bool {
    match std::env::var(REQUIRE_SANDBOX_ENV) {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

static UNSANDBOXED_WARNING_EMITTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Emit the unsandboxed-fallback warning, once per process. Returns `true`
/// when this call emitted it (used by tests; later calls are no-ops).
///
/// Deliberately written to stderr in addition to `tracing`: embedders that
/// never install a tracing subscriber must still see it.
fn warn_unsandboxed_once() -> bool {
    if UNSANDBOXED_WARNING_EMITTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    eprintln!(
        "kernex-sandbox: WARNING: OS-level sandbox enforcement is unavailable on this host; \
         subprocesses run UNSANDBOXED (code-level path checks remain). \
         Set {REQUIRE_SANDBOX_ENV}=1 or SandboxProfile::require_os_enforcement to refuse \
         spawning instead. This fail-open fallback becomes fail-closed in kernex 1.0."
    );
    tracing::warn!(
        "OS-level sandbox enforcement unavailable; subprocesses run unsandboxed \
         (set {REQUIRE_SANDBOX_ENV}=1 to refuse)"
    );
    true
}

/// Shared fail-mode gate: error when enforcement is required but the host
/// cannot provide it; warn once per process when falling back unsandboxed.
fn check_enforcement(available: bool, profile: &SandboxProfile) -> std::io::Result<()> {
    if available {
        return Ok(());
    }
    if enforcement_required(profile) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "OS-level sandbox enforcement is required \
                 (SandboxProfile::require_os_enforcement or {REQUIRE_SANDBOX_ENV}=1) \
                 but unavailable on this host"
            ),
        ));
    }
    warn_unsandboxed_once();
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use tracing::warn;

mod env;
mod gate;
mod paths;
mod policy;
mod scope;
mod tool_access;

pub use env::{hardened_env, BASE_ENV_ALLOWLIST};
pub use paths::{credential_read_deny_dirs, write_allow_dirs};
pub use scope::SandboxScope;
pub use tool_access::{
    no_follow, resolved_path, tool_read_path, tool_search_exclusions, tool_write_path,
    ToolPathDenied,
};

use gate::check_workspace;
use policy::Policy;

#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "linux")]
mod landlock_sandbox;

/// Build a [`Command`] with OS-level system protection (best-effort).
///
/// The subprocess may write under the scope's workspace and the other write
/// roots, and nowhere in the runtime data directory. On platforms without
/// OS-level enforcement (no `/usr/bin/sandbox-exec`, no Landlock kernel
/// support, Windows, etc.) this falls back to a plain command after a
/// once-per-process stderr warning, preserving the historical fail-open
/// behaviour. This function is infallible by signature and therefore cannot
/// honour [`REQUIRE_SANDBOX_ENV`] or `require_os_enforcement`; deployments
/// where running unsandboxed is unacceptable must use
/// [`try_protected_command`].
///
/// When the scope or a `blocked_paths` entry is not an absolute path the
/// policy cannot be built; the returned command then fails to spawn
/// instead of running unprotected.
pub fn protected_command(program: &str, scope: &SandboxScope, profile: &SandboxProfile) -> Command {
    if !os_enforcement_available() {
        warn_unsandboxed_once();
    }
    let policy = Policy::new(scope, profile);
    if let Some(problem) = policy.invalid() {
        return gate::refused_command(problem);
    }
    if let Some(rejection) = policy.workspace_rejection() {
        gate::warn_workspace_rejected_once(&scope.workspace, rejection);
    }
    let mut cmd = match platform_command(program, &policy, profile) {
        Ok(cmd) => cmd,
        Err(e) => return gate::refused_command(&e.to_string()),
    };
    hardened_env(&mut cmd);
    cmd
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
/// Returns an [`io::Error`](std::io::Error) when enforcement is required,
/// either via `profile.require_os_enforcement` or the
/// [`REQUIRE_SANDBOX_ENV`] environment variable, and one of these holds:
///
/// - the host cannot apply OS-level enforcement
///   ([`Unsupported`](std::io::ErrorKind::Unsupported));
/// - the scope's workspace is not accepted as a write root, because it is the
///   home directory, one of its ancestors, `/`, or inside a protected
///   directory ([`InvalidInput`](std::io::ErrorKind::InvalidInput)).
///
/// A scope directory or a `blocked_paths` entry that is not an absolute
/// path is always an [`InvalidInput`](std::io::ErrorKind::InvalidInput)
/// error, whether or not enforcement is required: a path that cannot be
/// resolved cannot be protected.
///
/// When enforcement is not required this behaves identically to
/// [`protected_command`] (warns once per process and falls back) and always
/// returns `Ok(Command)`.
pub fn try_protected_command(
    program: &str,
    scope: &SandboxScope,
    profile: &SandboxProfile,
) -> std::io::Result<Command> {
    check_enforcement(os_enforcement_available(), profile)?;
    let policy = Policy::new(scope, profile);
    gate::check_policy(&policy)?;
    check_workspace(&policy, scope, profile)?;
    let mut cmd = platform_command(program, &policy, profile)?;
    hardened_env(&mut cmd);
    Ok(cmd)
}

/// Returns `true` when the current host can apply OS-level sandbox
/// enforcement (Seatbelt on macOS or Landlock on Linux). Used by
/// [`try_protected_command`] to honour `require_os_enforcement`.
///
/// On Linux this needs Landlock ABI 3 (kernel 6.2): an older kernel cannot
/// stop a file outside the write roots from being truncated. The rules such
/// a kernel supports are still applied when enforcement is not required.
pub fn os_enforcement_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(target_os = "linux")]
    {
        landlock_sandbox::enforces()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// The Landlock ABI version of the running kernel, read with the
/// `landlock_create_ruleset` syscall. `None` when Landlock is unavailable,
/// and on every platform other than Linux.
///
/// The ABI tells how deep enforcement goes: 4 and later restrict TCP, 6 and
/// later scope abstract Unix sockets and signals.
pub fn landlock_abi_version() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        landlock_sandbox::abi_version()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Check if a write to the given path should be blocked.
///
/// Returns `true` unless the path is under one of the scope's write roots
/// (the workspace, the temp directories, the toolchain cache directories,
/// `allowed_paths`) and outside every protected location (the runtime data
/// directory's database, skills, projects and configuration, the system
/// directories, `blocked_paths`). A relative path is always blocked.
///
/// Symlinks are resolved before comparison, including in the parent of a
/// file that does not exist yet. Used by tool executors for code-level
/// enforcement.
pub fn is_write_blocked(path: &Path, scope: &SandboxScope, profile: &SandboxProfile) -> bool {
    Policy::new(scope, profile).check_write(path).is_err()
}

/// Check if a read from the given path should be blocked.
///
/// Returns `true` if the path targets a protected location: the memory
/// database, the configuration file, a credential store under `$HOME`, or a
/// `blocked_paths` entry. A relative path is always blocked.
///
/// Symlinks are resolved before comparison. Used by tool executors for
/// code-level enforcement.
pub fn is_read_blocked(path: &Path, scope: &SandboxScope, profile: &SandboxProfile) -> bool {
    Policy::new(scope, profile).check_read(path).is_err()
}

#[cfg(target_os = "macos")]
fn platform_command(
    program: &str,
    policy: &Policy,
    profile: &SandboxProfile,
) -> std::io::Result<Command> {
    Ok(seatbelt::protected_command(program, policy, profile))
}

#[cfg(target_os = "linux")]
fn platform_command(
    program: &str,
    policy: &Policy,
    profile: &SandboxProfile,
) -> std::io::Result<Command> {
    landlock_sandbox::protected_command(program, policy, profile)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_command(
    program: &str,
    _policy: &Policy,
    _profile: &SandboxProfile,
) -> std::io::Result<Command> {
    warn!("OS-level protection not available on this platform; using code-level enforcement only");
    Ok(Command::new(program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scope on paths that exist on no host, so the policy leaves them
    /// as written.
    fn test_scope() -> SandboxScope {
        SandboxScope::new("/kx-lib-test/ws", "/kx-lib-test/state")
    }

    #[test]
    fn test_protected_command_returns_command() {
        let profile = SandboxProfile::default();
        let cmd = protected_command("claude", &test_scope(), &profile);
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert!(!program.is_empty());
    }

    #[test]
    fn test_protected_command_hardens_env() {
        // A parent var outside the allowlist must not survive; the base
        // allowlist (PATH) and the LC_* family must.
        std::env::set_var("KERNEX_TEST_SENTINEL_DO_NOT_PASS", "1");
        std::env::set_var("LC_KERNEX_TEST", "es_ES.UTF-8");

        let profile = SandboxProfile::default();
        let cmd = protected_command("echo", &test_scope(), &profile);
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
    fn test_is_write_blocked_follows_the_policy() {
        let scope = test_scope();
        let profile = SandboxProfile::default();
        assert!(!is_write_blocked(
            Path::new("/kx-lib-test/ws/src/main.rs"),
            &scope,
            &profile
        ));
        assert!(is_write_blocked(
            Path::new("/kx-lib-test/other/file"),
            &scope,
            &profile
        ));
        assert!(is_write_blocked(
            Path::new("/kx-lib-test/state/memory.db"),
            &scope,
            &profile
        ));
        assert!(is_write_blocked(
            Path::new("/kx-lib-test/state/skills/x/SKILL.md"),
            &scope,
            &profile
        ));
        assert!(is_write_blocked(Path::new("/bin/sh"), &scope, &profile));
        assert!(is_write_blocked(
            Path::new("relative/path"),
            &scope,
            &profile
        ));
    }

    #[test]
    fn test_is_read_blocked_follows_the_policy() {
        let scope = test_scope().with_config_path("/kx-lib-test/etc/config.toml");
        let profile = SandboxProfile::default();
        assert!(!is_read_blocked(
            Path::new("/kx-lib-test/other/README.md"),
            &scope,
            &profile
        ));
        assert!(!is_read_blocked(
            Path::new("/kx-lib-test/state/skills/x/SKILL.md"),
            &scope,
            &profile
        ));
        assert!(is_read_blocked(
            Path::new("/kx-lib-test/state/memory.db"),
            &scope,
            &profile
        ));
        assert!(is_read_blocked(
            Path::new("/kx-lib-test/state/config.toml"),
            &scope,
            &profile
        ));
        assert!(is_read_blocked(
            Path::new("/kx-lib-test/etc/config.toml"),
            &scope,
            &profile
        ));
        assert!(is_read_blocked(
            Path::new("relative/path"),
            &scope,
            &profile
        ));
    }

    #[test]
    fn test_blocked_paths_apply_to_reads_and_writes() {
        let scope = test_scope();
        let profile = SandboxProfile::new()
            .with_blocked_paths(vec![PathBuf::from("/kx-lib-test/ws/secrets")]);
        let target = Path::new("/kx-lib-test/ws/secrets/token");
        assert!(is_read_blocked(target, &scope, &profile));
        assert!(is_write_blocked(target, &scope, &profile));
    }

    #[test]
    fn test_rejected_workspace_is_refused_when_enforcement_is_required() {
        let scope = SandboxScope::new("/", "/kx-lib-test/state");
        let strict = SandboxProfile::new().with_require_os_enforcement(true);
        let policy = Policy::new(&scope, &strict);
        let err = check_workspace(&policy, &scope, &strict).expect_err("must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

        // Not required: falls back (warns once) unless the developer's shell
        // exports the env var.
        if std::env::var_os(REQUIRE_SANDBOX_ENV).is_none() {
            let lenient = SandboxProfile::default();
            let policy = Policy::new(&scope, &lenient);
            assert!(check_workspace(&policy, &scope, &lenient).is_ok());
        }
    }

    #[test]
    fn test_accepted_workspace_passes_the_workspace_check() {
        let scope = test_scope();
        let strict = SandboxProfile::new().with_require_os_enforcement(true);
        let policy = Policy::new(&scope, &strict);
        assert!(check_workspace(&policy, &scope, &strict).is_ok());
    }

    #[test]
    fn test_try_protected_command_lenient_returns_ok() {
        // require_os_enforcement = false → always Ok, even when the platform
        // cannot enforce.
        let profile = SandboxProfile::default();
        let result = try_protected_command("claude", &test_scope(), &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_protected_command_strict_matches_capability() {
        // require_os_enforcement = true → result depends on host capability.
        // On a host that supports enforcement we expect Ok; on one that does
        // not we expect Err(ErrorKind::Unsupported). This guards against the
        // strict flag silently being ignored.
        let profile = SandboxProfile::new().with_require_os_enforcement(true);
        let result = try_protected_command("claude", &test_scope(), &profile);
        if os_enforcement_available() {
            assert!(result.is_ok(), "expected Ok on a host with enforcement");
        } else {
            let err = result.expect_err("expected Err on a host without enforcement");
            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        }
    }

    #[test]
    fn test_enforcement_required_profile_flag() {
        let lenient = SandboxProfile::default();
        let strict = SandboxProfile {
            require_os_enforcement: true,
            ..Default::default()
        };
        // The env var may be set in the developer's shell; only assert the
        // cases that are deterministic regardless of it.
        assert!(enforcement_required(&strict));
        if std::env::var_os(REQUIRE_SANDBOX_ENV).is_none() {
            assert!(!enforcement_required(&lenient));
        }
    }

    #[test]
    fn test_env_requires_sandbox_value_parsing() {
        // Exercises the value parser via a save/restore cycle. Runs in one
        // test to avoid interleaving env mutation across parallel tests.
        let saved = std::env::var_os(REQUIRE_SANDBOX_ENV);

        std::env::set_var(REQUIRE_SANDBOX_ENV, "1");
        assert!(env_requires_sandbox());
        std::env::set_var(REQUIRE_SANDBOX_ENV, "true");
        assert!(env_requires_sandbox());
        std::env::set_var(REQUIRE_SANDBOX_ENV, "0");
        assert!(!env_requires_sandbox());
        std::env::set_var(REQUIRE_SANDBOX_ENV, "false");
        assert!(!env_requires_sandbox());
        std::env::set_var(REQUIRE_SANDBOX_ENV, "");
        assert!(!env_requires_sandbox());
        std::env::remove_var(REQUIRE_SANDBOX_ENV);
        assert!(!env_requires_sandbox());

        if let Some(v) = saved {
            std::env::set_var(REQUIRE_SANDBOX_ENV, v);
        }
    }

    #[test]
    fn test_check_enforcement_fail_modes() {
        let lenient = SandboxProfile::default();
        let strict = SandboxProfile {
            require_os_enforcement: true,
            ..Default::default()
        };

        // Enforcement available: always Ok, strict or not.
        assert!(check_enforcement(true, &lenient).is_ok());
        assert!(check_enforcement(true, &strict).is_ok());

        // Unavailable + required: refuse with ErrorKind::Unsupported.
        let err = check_enforcement(false, &strict).expect_err("strict must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains(REQUIRE_SANDBOX_ENV),
            "refusal message must name the env-var escape hatch"
        );

        // Unavailable + not required: fall back Ok (warns once per process)
        // unless the developer's shell exports the env var.
        if std::env::var_os(REQUIRE_SANDBOX_ENV).is_none() {
            assert!(check_enforcement(false, &lenient).is_ok());
        }
    }

    #[test]
    fn test_warn_unsandboxed_emits_once_per_process() {
        // First call in the process may or may not have happened already
        // (other tests hit the fallback path); what must hold is that after
        // one emission every subsequent call is a no-op.
        let first = warn_unsandboxed_once();
        let second = warn_unsandboxed_once();
        if first {
            assert!(!second, "warning emitted twice in one process");
        } else {
            assert!(!second, "warning re-emitted after a prior emission");
        }
    }
}
