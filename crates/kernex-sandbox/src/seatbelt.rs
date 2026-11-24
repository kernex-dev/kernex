//! macOS Seatbelt (sandbox-exec) enforcement.
//!
//! The profile is generated from the shared [`Policy`] and denies by default.
//! A subprocess may run programs, read files outside the read-deny paths,
//! write under the write roots and look up a short list of system services.
//! Everything else is refused: the keychain, other applications, Apple
//! events, pseudo-terminals, and the network unless the profile allows it.
//!
//! No path is ever written into the profile text. Each one is passed to
//! `sandbox-exec` as a `-D` parameter and referenced as `(param "P<n>")`, so
//! a path containing quotes or parentheses can neither break the profile nor
//! drop out of it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::warn;

use crate::policy::Policy;

/// Path to the sandbox-exec binary (built into macOS).
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// A generated profile and the path parameters it refers to.
struct BuiltProfile {
    text: String,
    params: Vec<(String, PathBuf)>,
}

impl BuiltProfile {
    /// Register `path` as a parameter and return the SBPL expression for it.
    fn param(&mut self, path: &Path) -> String {
        let name = format!("P{}", self.params.len());
        let expr = format!("(param \"{name}\")");
        self.params.push((name, path.to_path_buf()));
        expr
    }

    /// Render `paths` as `(<filter> (param ...))` lines. `subpath` matches a
    /// path and everything below it; `literal` matches the path alone.
    fn filters(&mut self, filter: &str, paths: &[PathBuf]) -> String {
        let mut out = String::new();
        for path in paths {
            let param = self.param(path);
            out.push_str(&format!("  ({filter} {param})\n"));
        }
        out
    }

    /// Append `(<action> <operation> <filters>)`. An empty filter list emits
    /// nothing: an operation with no filter would match every path.
    fn rule(&mut self, action: &str, operation: &str, filters: &str) {
        if !filters.is_empty() {
            self.text
                .push_str(&format!("({action} {operation}\n{filters})\n"));
        }
    }
}

/// What every subprocess gets: process control inside the sandbox, read-only
/// system information, and the services a dynamically linked program needs
/// to resolve users, log and find its temp directory.
const BASE_RULES: &str = r#"(version 1)
(deny default)
(allow process-fork)
(allow process-exec*)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow system-info)
(allow ipc-posix-sem)
(allow ipc-posix-shm*)
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.system.opendirectoryd.membership")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.logger")
  (global-name "com.apple.logd")
  (global-name "com.apple.diagnosticd")
  (global-name "com.apple.bsd.dirhelper")
)
(allow file-read*)
(allow file-write-data
  (literal "/dev/null")
  (literal "/dev/zero")
  (literal "/dev/tty")
  (literal "/dev/dtracehelper")
  (literal "/dev/stdout")
  (literal "/dev/stderr")
  (regex #"^/dev/fd/[0-9]+$")
)
(allow file-ioctl (literal "/dev/dtracehelper"))
"#;

/// Added when the profile allows the network: IP sockets, plus the services
/// that resolve names and evaluate certificate trust. Local sockets stay
/// closed except the resolver's: a daemon listening on one (a container
/// runtime, an SSH agent) would otherwise be one `connect` away.
const NETWORK_RULES: &str = r#"(allow network-outbound (remote ip))
(allow network-inbound (local ip))
(allow network-bind (local ip))
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
(allow system-socket)
(allow mach-lookup
  (global-name "com.apple.dnssd.service")
  (global-name "com.apple.SystemConfiguration.configd")
  (global-name "com.apple.SystemConfiguration.DNSConfiguration")
  (global-name "com.apple.trustd")
  (global-name "com.apple.trustd.agent")
  (global-name "com.apple.TrustEvaluationAgent")
)
"#;

/// Added when the profile allows keychain queries.
const KEYCHAIN_RULES: &str = r#"(allow mach-lookup
  (global-name "com.apple.SecurityServer")
  (global-name "com.apple.securityd.xpc")
  (global-name "com.apple.security.agent")
  (global-name "com.apple.secd")
)
"#;

/// Generate the Seatbelt profile for `policy`.
///
/// SBPL is last-match-wins, so the order is: the base rules, the write
/// roots, then the protected paths. A protected path inside a write root
/// therefore stays denied, and so does renaming or removing a directory
/// above it (the policy's guards).
fn build_profile(policy: &Policy, profile: &crate::SandboxProfile) -> BuiltProfile {
    let mut built = BuiltProfile {
        text: String::from(BASE_RULES),
        params: Vec::new(),
    };
    // Without these no socket of any family can be used, the resolver's
    // included, so DNS is unavailable too.
    if profile.allow_network {
        built.text.push_str(NETWORK_RULES);
    }
    if profile.allow_keychain {
        built.text.push_str(KEYCHAIN_RULES);
    }

    let roots = built.filters("subpath", policy.write_roots());
    built.rule("allow", "file-write*", &roots);

    let write_deny = built.filters("subpath", policy.write_deny());
    built.rule("deny", "file-write*", &write_deny);
    // Creating, removing and renaming a guard; its contents, timestamps and
    // mode stay writable.
    let guards = built.filters("literal", policy.guards());
    built.rule("deny", "file-write-create file-write-unlink", &guards);

    let read_deny = built.filters("subpath", policy.read_deny());
    built.rule("deny", "file-read*", &read_deny);

    built
}

/// Build a [`Command`] wrapped with `sandbox-exec` and the profile generated
/// from `policy`.
///
/// If `/usr/bin/sandbox-exec` does not exist, logs a warning and returns
/// a plain command without OS-level enforcement.
pub(crate) fn protected_command(
    program: &str,
    policy: &Policy,
    profile: &crate::SandboxProfile,
) -> Command {
    if !Path::new(SANDBOX_EXEC).exists() {
        warn!("sandbox-exec not found at {SANDBOX_EXEC}; falling back to code-level protection");
        return Command::new(program);
    }

    let built = build_profile(policy, profile);
    let mut cmd = Command::new(SANDBOX_EXEC);
    for (name, path) in &built.params {
        let mut definition = OsString::from(format!("{name}="));
        definition.push(path);
        cmd.arg("-D").arg(definition);
    }
    cmd.arg("-p").arg(built.text).arg("--").arg(program);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::HostDirs;
    use crate::{SandboxProfile, SandboxScope};

    // Paths that exist on no host, so the policy leaves them as written.
    const HOME: &str = "/kx-sbpl-test/home/ana";

    fn host() -> HostDirs {
        HostDirs {
            home: PathBuf::from(HOME),
            temp_dirs: vec![PathBuf::from("/kx-sbpl-test/tmp")],
        }
    }

    fn policy_for(profile: &SandboxProfile) -> Policy {
        let scope = SandboxScope::new(format!("{HOME}/code/proj"), format!("{HOME}/.kernex"));
        Policy::with_host(&scope, profile, &host())
    }

    /// The profile with every parameter replaced by its quoted path, which
    /// is what `sandbox-exec` evaluates.
    fn rendered(built: &BuiltProfile) -> String {
        let mut text = built.text.clone();
        for (name, path) in built.params.iter().rev() {
            text = text.replace(
                &format!("(param \"{name}\")"),
                &format!("\"{}\"", path.display()),
            );
        }
        text
    }

    fn default_profile_text() -> String {
        let profile = SandboxProfile::default();
        rendered(&build_profile(&policy_for(&profile), &profile))
    }

    /// The text of the last rule that starts with `head`.
    fn last_rule<'a>(profile: &'a str, head: &str) -> &'a str {
        let start = profile.rfind(head).unwrap();
        let end = profile[start..].find("\n)\n").unwrap();
        &profile[start..start + end]
    }

    #[test]
    fn test_profile_denies_by_default() {
        let profile = default_profile_text();
        assert!(profile.contains("(deny default)"));
        assert!(!profile.contains("(allow default)"));
    }

    #[test]
    fn test_profile_grants_no_keychain_service_by_default() {
        let profile = default_profile_text();
        assert!(!profile.contains("SecurityServer"), "{profile}");
        let profile_obj = SandboxProfile::new().with_allow_keychain(true);
        let profile = rendered(&build_profile(&policy_for(&profile_obj), &profile_obj));
        assert!(profile.contains("com.apple.SecurityServer"));
    }

    #[test]
    fn test_toolchain_bin_dirs_are_write_denied() {
        let profile = default_profile_text();
        let deny = last_rule(&profile, "(deny file-write*");
        for dir in [
            ".cargo/bin",
            ".cargo/env",
            ".bun/bin",
            ".deno/bin",
            ".local/bin",
        ] {
            assert!(
                deny.contains(&format!("(subpath \"{HOME}/{dir}\")")),
                "{dir}"
            );
        }
        // The directory above stays writable, and cannot be renamed.
        let guards = last_rule(&profile, "(deny file-write-create file-write-unlink");
        assert!(guards.contains(&format!("(literal \"{HOME}/.cargo\")")));
    }

    #[test]
    fn test_profile_blocks_system_dirs() {
        let profile = default_profile_text();
        let deny = last_rule(&profile, "(deny file-write*");
        for dir in [
            "/System",
            "/bin",
            "/sbin",
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/usr/libexec",
            "/private/etc",
            "/Library",
        ] {
            assert!(deny.contains(&format!("(subpath \"{dir}\")")), "{dir}");
        }
    }

    #[test]
    fn test_only_the_write_roots_are_writable() {
        // Nothing outside the roots is granted: no rule names the home
        // directory, `/usr/local` or `/opt`.
        let profile = default_profile_text();
        assert_eq!(profile.matches("(allow file-write*").count(), 1);
        let allow = last_rule(&profile, "(allow file-write*");
        for path in [HOME, "/usr/local", "/opt", "/"] {
            assert!(!allow.contains(&format!("(subpath \"{path}\")")), "{path}");
        }
    }

    #[test]
    fn test_profile_leaves_device_files_writable() {
        // A subprocess must keep writing /dev/null and its terminal.
        let profile = default_profile_text();
        assert!(!profile.contains("\"/dev\""), "{profile}");
    }

    #[test]
    fn test_profile_locks_down_home_writes() {
        let profile = default_profile_text();
        let allow = last_rule(&profile, "(allow file-write*");
        assert!(allow.contains(&format!("(subpath \"{HOME}/code/proj\")")));
        assert!(allow.contains(r#"(subpath "/kx-sbpl-test/tmp")"#));
        assert!(allow.contains(&format!("(subpath \"{HOME}/.cargo\")")));
    }

    #[test]
    fn test_data_dir_is_not_writable() {
        let profile = default_profile_text();
        let allow = last_rule(&profile, "(allow file-write*");
        assert!(
            !allow.contains(&format!("(subpath \"{HOME}/.kernex\")")),
            "data dir must not be a write root: {allow}"
        );
    }

    #[test]
    fn test_protected_state_is_denied_after_the_roots() {
        let profile = default_profile_text();
        let allow_pos = profile.find("(allow file-write*").unwrap();
        let deny_pos = profile.rfind("(deny file-write*").unwrap();
        assert!(
            allow_pos < deny_pos,
            "write-deny must be the last write rule"
        );

        let deny = last_rule(&profile, "(deny file-write*");
        for path in [
            ".kernex/memory.db",
            ".kernex/memory.db-wal",
            ".kernex/data",
            ".kernex/config.toml",
            ".kernex/skills",
            ".kernex/projects",
        ] {
            assert!(
                deny.contains(&format!("(subpath \"{HOME}/{path}\")")),
                "{path}"
            );
        }
    }

    #[test]
    fn test_directories_above_protected_state_cannot_be_renamed() {
        // State inside the workspace: the workspace and the state directory
        // are denied as literals, so neither can be moved out from under the
        // deny rules, while their other contents stay writable.
        let scope = SandboxScope::new(
            format!("{HOME}/code/proj"),
            format!("{HOME}/code/proj/.agent"),
        );
        let profile_obj = SandboxProfile::default();
        let policy = Policy::with_host(&scope, &profile_obj, &host());
        let profile = rendered(&build_profile(&policy, &profile_obj));
        let guards = last_rule(&profile, "(deny file-write-create file-write-unlink");
        assert!(guards.contains(&format!("(literal \"{HOME}/code/proj\")")));
        assert!(guards.contains(&format!("(literal \"{HOME}/code/proj/.agent\")")));
        let deny = last_rule(&profile, "(deny file-write*");
        assert!(!deny.contains(&format!("(subpath \"{HOME}/code/proj\")")));
        assert!(!deny.contains(&format!("(literal \"{HOME}/code/proj\")")));
    }

    #[test]
    fn test_profile_denies_state_and_credential_reads() {
        let profile = default_profile_text();
        let deny = last_rule(&profile, "(deny file-read*");
        for path in [
            ".kernex/memory.db",
            ".kernex/data",
            ".kernex/config.toml",
            ".ssh",
            ".aws",
            ".npmrc",
            ".git-credentials",
            ".cargo/credentials.toml",
            ".claude",
            ".zsh_history",
            "Library/Keychains",
        ] {
            assert!(
                deny.contains(&format!("(subpath \"{HOME}/{path}\")")),
                "{path}"
            );
        }
        assert!(
            !deny.contains(&format!("(subpath \"{HOME}/.kernex/skills\")")),
            "skills stay readable"
        );
    }

    #[test]
    fn test_rejected_workspace_is_not_a_write_root() {
        let scope = SandboxScope::new(HOME, format!("{HOME}/.kernex"));
        let profile_obj = SandboxProfile::default();
        let policy = Policy::with_host(&scope, &profile_obj, &host());
        let profile = rendered(&build_profile(&policy, &profile_obj));
        let allow = last_rule(&profile, "(allow file-write*");
        assert!(
            !allow.contains(&format!("(subpath \"{HOME}\")")),
            "home must not be re-allowed: {allow}"
        );
    }

    #[test]
    fn test_empty_filter_list_emits_no_rule() {
        let mut built = BuiltProfile {
            text: String::new(),
            params: Vec::new(),
        };
        built.rule("deny", "file-read*", "");
        assert_eq!(built.text, "");
        built.rule("deny", "file-read*", "  (subpath \"/x\")\n");
        assert_eq!(built.text, "(deny file-read*\n  (subpath \"/x\")\n)\n");
    }

    #[test]
    fn test_hostile_path_never_reaches_the_profile_text() {
        // A path containing `"` and `)` would inject policy code if it were
        // interpolated. It travels as a parameter instead, and stays denied.
        let hostile = PathBuf::from("/kx-sbpl-test/evil\") (allow file-write* (subpath \"/");
        let profile_obj = SandboxProfile::new().with_blocked_paths(vec![hostile.clone()]);
        let built = build_profile(&policy_for(&profile_obj), &profile_obj);
        assert!(
            !built.text.contains("evil"),
            "path text leaked into the profile: {}",
            built.text
        );
        assert!(built.params.iter().any(|(_, path)| path == &hostile));
        // Every parameter the text refers to is defined exactly once.
        for (name, _) in &built.params {
            let reference = format!("(param \"{name}\")");
            assert_eq!(built.text.matches(&reference).count(), 1, "{name}");
        }
    }

    #[test]
    fn test_command_passes_every_parameter() {
        let profile = SandboxProfile::default();
        let policy = policy_for(&profile);
        let expected = build_profile(&policy, &profile).params.len();
        let cmd = protected_command("claude", &policy, &profile);
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_string_lossy().to_string();
        if program.contains("sandbox-exec") {
            let definitions = std_cmd.get_args().filter(|arg| *arg == "-D").count();
            assert_eq!(definitions, expected);
        } else {
            assert_eq!(program, "claude");
        }
    }

    #[test]
    fn test_profile_denies_network_by_default() {
        let profile = default_profile_text();
        assert!(
            !profile.contains("network") && !profile.contains("system-socket"),
            "default profile must grant no socket access: {profile}"
        );
    }

    #[test]
    fn test_profile_allows_network_on_opt_in() {
        let profile_obj = SandboxProfile::new().with_allow_network(true);
        let profile = rendered(&build_profile(&policy_for(&profile_obj), &profile_obj));
        assert!(
            profile.contains("(allow network-outbound (remote ip))"),
            "opt-in profile must allow the network: {profile}"
        );
    }
}
