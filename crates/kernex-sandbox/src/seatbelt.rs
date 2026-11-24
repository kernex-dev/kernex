//! macOS Seatbelt (sandbox-exec) enforcement.
//!
//! The profile is generated from the shared [`Policy`]: writes inside `$HOME`
//! are denied outside the policy's write roots, the write-deny and read-deny
//! paths are denied on top, and everything else is allowed by default.
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

/// Generate the Seatbelt profile for `policy`.
///
/// SBPL is last-match-wins, so the order is: deny writes inside `$HOME`,
/// re-allow the write roots, then deny the protected paths. A protected path
/// inside a write root therefore stays denied, and so does renaming or
/// removing a directory above it (the policy's guards).
fn build_profile(policy: &Policy, profile: &crate::SandboxProfile) -> BuiltProfile {
    let mut built = BuiltProfile {
        text: String::from("(version 1)\n(allow default)\n"),
        params: Vec::new(),
    };

    // $HOME write lockdown. Writes outside $HOME keep allow-default
    // behaviour, minus the write-deny list below.
    let home = built.filters("subpath", &[policy.home().to_path_buf()]);
    built.rule("deny", "file-write*", &home);
    let roots = built.filters("subpath", policy.write_roots());
    built.rule("allow", "file-write*", &roots);

    let mut write_deny = built.filters("subpath", policy.write_deny());
    write_deny.push_str(&built.filters("literal", policy.guards()));
    built.rule("deny", "file-write*", &write_deny);

    let read_deny = built.filters("subpath", policy.read_deny());
    built.rule("deny", "file-read*", &read_deny);

    // Network egress deny-by-default: `network*` covers outbound, inbound,
    // and bind for every socket family (TCP, UDP, and local sockets, which
    // also disables DNS via mDNSResponder). Tools that genuinely need the
    // network opt in via `SandboxProfile::allow_network` (wired from the
    // per-tool `network = true` declaration).
    if !profile.allow_network {
        built.text.push_str("(deny network*)\n");
    }

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
    fn test_profile_allows_by_default() {
        assert!(default_profile_text().contains("(allow default)"));
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
        assert!(
            !profile.contains(r#"(subpath "/usr/local")"#),
            "/usr/local should not be blocked"
        );
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
        let home_deny = format!("(deny file-write*\n  (subpath \"{HOME}\")\n)");
        let home_pos = profile.find(&home_deny).expect("home write deny");
        let allow_pos = profile.find("(allow file-write*").expect("write roots");
        assert!(home_pos < allow_pos, "roots must follow the home deny");

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
        let deny = last_rule(&profile, "(deny file-write*");
        assert!(deny.contains(&format!("(literal \"{HOME}/code/proj\")")));
        assert!(deny.contains(&format!("(literal \"{HOME}/code/proj/.agent\")")));
        assert!(!deny.contains(&format!("(subpath \"{HOME}/code/proj\")")));
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
            profile.contains("(deny network*)"),
            "default profile must deny network egress: {profile}"
        );
    }

    #[test]
    fn test_profile_allows_network_on_opt_in() {
        let profile_obj = SandboxProfile::new().with_allow_network(true);
        let profile = rendered(&build_profile(&policy_for(&profile_obj), &profile_obj));
        assert!(
            !profile.contains("(deny network*)"),
            "opt-in profile must not deny network: {profile}"
        );
    }
}
