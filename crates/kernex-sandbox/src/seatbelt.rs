//! macOS Seatbelt (sandbox-exec) enforcement — blocklist approach.
//!
//! Denies writes to dangerous system directories and the runtime's core database.
//! Denies reads to the runtime's core data directory and config file.
//! Everything else is allowed by default.

use std::path::Path;
use tokio::process::Command;
use tracing::warn;

/// Path to the sandbox-exec binary (built into macOS).
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Returns the SBPL-safe form of `path` if it can be safely embedded in a
/// quoted profile string, or `None` if the path contains characters that
/// could break out of the string literal or terminate an enclosing form.
///
/// SBPL string literals are double-quoted with no escape mechanism; a `"`,
/// `)`, `\\`, or newline inside a path string injects raw policy code. Only
/// absolute paths are accepted because relative paths in a profile are
/// nonsensical and indicate caller confusion.
///
/// `;` and `*` are also rejected as a defence-in-depth measure: today the
/// path is always interpolated inside a string literal where these are
/// inert, but `;` is the SBPL line-comment leader and `*` carries glob
/// semantics in some forms — if a future edit ever interpolates a path
/// outside string quotes, these characters would let an attacker comment
/// out the rest of the policy.
fn sanitize_sbpl_path(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let s = path.to_str()?;
    if s.bytes().any(|b| {
        matches!(
            b,
            b'"' | b'\\' | b'(' | b')' | b'\n' | b'\r' | 0 | b';' | b'*'
        )
    }) {
        return None;
    }
    Some(s.to_string())
}

/// Generate a Seatbelt profile that blocks writes and reads to dangerous locations.
///
/// Blocklist approach: allow everything, deny specific dangerous paths.
/// `data_dir` is the runtime data directory (e.g. `~/.kernex/`).
/// - Writes to system dirs and `{data_dir}/data/` are denied.
/// - Reads to `{data_dir}/data/` and `{data_dir}/config.toml` are denied.
///
/// Paths that fail [`sanitize_sbpl_path`] are silently dropped with a warning
/// rather than interpolated into the policy — preventing SBPL injection via a
/// hostile path containing `"` or `)`.
fn build_profile(data_dir: &Path, profile: &crate::SandboxProfile) -> String {
    let mut deny_writes = String::from(
        r#"  (subpath "/System")
  (subpath "/bin")
  (subpath "/sbin")
  (subpath "/usr/bin")
  (subpath "/usr/sbin")
  (subpath "/usr/lib")
  (subpath "/usr/libexec")
  (subpath "/private/etc")
  (subpath "/Library")
"#,
    );
    let mut deny_reads = String::new();

    if let Some(s) = sanitize_sbpl_path(&data_dir.join("data")) {
        deny_writes.push_str(&format!("  (subpath \"{s}\")\n"));
        deny_reads.push_str(&format!("  (subpath \"{s}\")\n"));
    } else {
        warn!(
            data_dir = %data_dir.display(),
            "skipping data/ deny rule: path is not SBPL-safe"
        );
    }

    if let Some(s) = sanitize_sbpl_path(&data_dir.join("config.toml")) {
        deny_writes.push_str(&format!("  (literal \"{s}\")\n"));
        deny_reads.push_str(&format!("  (literal \"{s}\")\n"));
    } else {
        warn!(
            data_dir = %data_dir.display(),
            "skipping config.toml deny rule: path is not SBPL-safe"
        );
    }

    for blocked in &profile.blocked_paths {
        match sanitize_sbpl_path(blocked) {
            Some(s) => {
                deny_writes.push_str(&format!("  (subpath \"{s}\")\n"));
                deny_reads.push_str(&format!("  (subpath \"{s}\")\n"));
            }
            None => warn!(
                path = %blocked.display(),
                "skipping blocked_paths entry: path is not SBPL-safe"
            ),
        }
    }

    // Credential read-deny list: block reads of high-value secret
    // stores under $HOME. Reads stay broad everywhere else.
    let home = crate::resolved_home();
    for cred in crate::credential_read_deny_dirs(&home) {
        if let Some(s) = sanitize_sbpl_path(&cred) {
            deny_reads.push_str(&format!("  (subpath \"{s}\")\n"));
        }
    }

    // $HOME write lockdown: deny writes inside $HOME, then re-allow
    // the workspace/data dir, $KERNEX_DATA_DIR, the system temp dirs, and the
    // toolchain cache/state dirs. SBPL is last-match-wins, so the re-deny of
    // `{data_dir}/data` and config below must follow the allow block. Writes
    // outside $HOME keep allow-default behaviour (minus the system denies).
    let mut allow_writes = String::new();
    let mut home_write_deny = String::new();
    if let Some(h) = sanitize_sbpl_path(&home) {
        home_write_deny.push_str(&format!("  (subpath \"{h}\")\n"));

        if let Some(s) = sanitize_sbpl_path(data_dir) {
            allow_writes.push_str(&format!("  (subpath \"{s}\")\n"));
        }
        for tmp in ["/tmp", "/private/tmp", "/private/var/tmp", "/var/tmp"] {
            allow_writes.push_str(&format!("  (subpath \"{tmp}\")\n"));
        }
        if let Some(kdd) = std::env::var_os("KERNEX_DATA_DIR")
            .map(std::path::PathBuf::from)
            .as_deref()
            .and_then(sanitize_sbpl_path)
        {
            allow_writes.push_str(&format!("  (subpath \"{kdd}\")\n"));
        }
        for dir in crate::write_allow_dirs(&home) {
            if let Some(s) = sanitize_sbpl_path(&dir) {
                allow_writes.push_str(&format!("  (subpath \"{s}\")\n"));
            }
        }
    }

    let mut redeny_writes = String::new();
    if let Some(s) = sanitize_sbpl_path(&data_dir.join("data")) {
        redeny_writes.push_str(&format!("  (subpath \"{s}\")\n"));
    }
    if let Some(s) = sanitize_sbpl_path(&data_dir.join("config.toml")) {
        redeny_writes.push_str(&format!("  (literal \"{s}\")\n"));
    }

    let home_write_block = if home_write_deny.is_empty() {
        String::new()
    } else {
        format!(
            "(deny file-write*\n{home_write_deny})\n\
            (allow file-write*\n{allow_writes})\n\
            (deny file-write*\n{redeny_writes})\n"
        )
    };

    // Network egress deny-by-default: `network*` covers outbound, inbound,
    // and bind for every socket family (TCP, UDP, and local sockets, which
    // also disables DNS via mDNSResponder). Tools that genuinely need the
    // network opt in via `SandboxProfile::allow_network` (wired from the
    // per-tool `network = true` declaration).
    let network_block = if profile.allow_network {
        String::new()
    } else {
        String::from("(deny network*)\n")
    };

    format!(
        "(version 1)\n\
        (allow default)\n\
        (deny file-write*\n{deny_writes})\n\
        {home_write_block}\
        (deny file-read*\n{deny_reads})\n\
        {network_block}"
    )
}

/// Build a [`Command`] wrapped with `sandbox-exec` write and read restrictions.
///
/// Blocklist: denies writes to system directories + `{data_dir}/data/`;
/// denies reads to `{data_dir}/data/` and `{data_dir}/config.toml`.
/// Everything else (home dir, /tmp, /usr/local, etc.) is accessible.
///
/// If `/usr/bin/sandbox-exec` does not exist, logs a warning and returns
/// a plain command without OS-level enforcement.
pub(crate) fn protected_command(
    program: &str,
    data_dir: &Path,
    profile: &crate::SandboxProfile,
) -> Command {
    if !Path::new(SANDBOX_EXEC).exists() {
        warn!("sandbox-exec not found at {SANDBOX_EXEC}; falling back to code-level protection");
        return Command::new(program);
    }

    let built_profile = build_profile(data_dir, profile);
    let mut cmd = Command::new(SANDBOX_EXEC);
    cmd.arg("-p").arg(built_profile).arg("--").arg(program);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_profile_blocks_system_dirs() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(profile.contains("(deny file-write*"));
        assert!(profile.contains(r#"(subpath "/System")"#));
        assert!(profile.contains(r#"(subpath "/bin")"#));
        assert!(profile.contains(r#"(subpath "/sbin")"#));
        assert!(profile.contains(r#"(subpath "/usr/bin")"#));
        assert!(profile.contains(r#"(subpath "/usr/sbin")"#));
        assert!(profile.contains(r#"(subpath "/usr/lib")"#));
        assert!(profile.contains(r#"(subpath "/usr/libexec")"#));
        assert!(profile.contains(r#"(subpath "/private/etc")"#));
        assert!(profile.contains(r#"(subpath "/Library")"#));
    }

    #[test]
    fn test_profile_blocks_data_dir() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            profile.contains("/home/user/.kernex/data"),
            "should block data dir (memory.db)"
        );
    }

    #[test]
    fn test_profile_allows_usr_local() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            !profile.contains(r#"(subpath "/usr/local")"#),
            "/usr/local should not be blocked"
        );
    }

    #[test]
    fn test_profile_allows_by_default() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            profile.contains("(allow default)"),
            "should allow everything by default"
        );
    }

    #[test]
    fn test_profile_blocks_data_dir_reads() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            profile.contains("(deny file-read*"),
            "should have file-read* deny"
        );
        let read_deny_pos = profile.find("(deny file-read*").unwrap();
        let after_read = &profile[read_deny_pos..];
        assert!(
            after_read.contains("/home/user/.kernex/data"),
            "should block reads to data dir"
        );
    }

    #[test]
    fn test_profile_blocks_config_writes() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        let write_deny_pos = profile.find("(deny file-write*").unwrap();
        let read_deny_pos = profile.find("(deny file-read*").unwrap();
        let write_section = &profile[write_deny_pos..read_deny_pos];
        assert!(
            write_section.contains("config.toml"),
            "should block writes to config.toml"
        );
    }

    #[test]
    fn test_profile_blocks_config_reads() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile::default();
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            profile.contains(r#"(literal "/home/user/.kernex/config.toml")"#),
            "should block reads to config.toml"
        );
    }

    #[test]
    fn test_profile_denies_credential_reads() {
        // Uses the same resolved HOME build_profile uses, so the .ssh entry
        // lands in the read-deny section when it sanitizes cleanly (any
        // plain absolute home does).
        let home = crate::resolved_home();
        let data_dir = home.join(".kernex");
        let profile = build_profile(&data_dir, &crate::SandboxProfile::default());
        let read_pos = profile.find("(deny file-read*").unwrap();
        let read_section = &profile[read_pos..];
        if let Some(ssh) = sanitize_sbpl_path(&home.join(".ssh")) {
            assert!(
                read_section.contains(&ssh),
                "credential read-deny missing .ssh: {read_section}"
            );
        }
        assert!(crate::credential_read_deny_dirs(&home)
            .iter()
            .any(|c| c.ends_with(".ssh")));
    }

    #[test]
    fn test_profile_locks_down_home_writes() {
        let home = crate::resolved_home();
        let data_dir = home.join(".kernex");
        let profile = build_profile(&data_dir, &crate::SandboxProfile::default());
        // A $HOME write-deny + re-allow block exists; the data dir and the
        // toolchain cache dirs are re-allowed so real tools keep working.
        assert!(profile.contains("(allow file-write*"));
        if let Some(dd) = sanitize_sbpl_path(&data_dir) {
            assert!(profile.contains(&dd), "data dir not re-allowed for writes");
        }
        assert!(profile.contains(".cargo"));
    }

    #[test]
    fn test_blocked_path_with_quote_is_dropped() {
        // A path containing a literal `"` would close the SBPL string and
        // inject arbitrary policy code if interpolated. It must be dropped.
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile {
            blocked_paths: vec![PathBuf::from(
                "/tmp/evil\") (allow file-write* (subpath \"/",
            )],
            ..Default::default()
        };
        let profile = build_profile(&data_dir, &profile_obj);
        // The profile legitimately contains an `(allow file-write*` block now
        // (the $HOME write lockdown re-allows the workspace/cache dirs); the
        // injection check is that the malicious path text never made it in.
        assert!(
            !profile.contains("/tmp/evil"),
            "injected blocked_path escaped sanitization: {profile}"
        );
    }

    #[test]
    fn test_blocked_path_with_paren_is_dropped() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile {
            blocked_paths: vec![PathBuf::from("/tmp/has)paren")],
            ..Default::default()
        };
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(!profile.contains("/tmp/has)paren"));
    }

    #[test]
    fn test_blocked_path_with_newline_is_dropped() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile {
            blocked_paths: vec![PathBuf::from("/tmp/has\nnewline")],
            ..Default::default()
        };
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(!profile.contains("newline"));
    }

    #[test]
    fn test_relative_blocked_path_is_dropped() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile_obj = crate::SandboxProfile {
            blocked_paths: vec![PathBuf::from("relative/path")],
            ..Default::default()
        };
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(!profile.contains("relative/path"));
    }

    #[test]
    fn test_command_structure() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = crate::SandboxProfile::default();
        let cmd = protected_command("claude", &data_dir, &profile);
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert!(
            program.contains("sandbox-exec") || program.contains("claude"),
            "unexpected program: {program}"
        );
    }

    #[test]
    fn test_profile_denies_network_by_default() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = build_profile(&data_dir, &crate::SandboxProfile::default());
        assert!(
            profile.contains("(deny network*)"),
            "default profile must deny network egress: {profile}"
        );
    }

    #[test]
    fn test_profile_allows_network_on_opt_in() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile_obj = crate::SandboxProfile {
            allow_network: true,
            ..Default::default()
        };
        let profile = build_profile(&data_dir, &profile_obj);
        assert!(
            !profile.contains("(deny network*)"),
            "opt-in profile must not deny network: {profile}"
        );
    }
}
