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

/// Generate a Seatbelt profile that blocks writes and reads to dangerous locations.
///
/// Blocklist approach: allow everything, deny specific dangerous paths.
/// `data_dir` is the runtime data directory (e.g. `~/.kernex/`).
/// - Writes to system dirs and `{data_dir}/data/` are denied.
/// - Reads to `{data_dir}/data/` and `{data_dir}/config.toml` are denied.
fn build_profile(data_dir: &Path) -> String {
    let data_data = data_dir.join("data");
    let data_data_str = data_data.display();
    let config_file = data_dir.join("config.toml");
    let config_str = config_file.display();

    format!(
        r#"(version 1)
(allow default)
(deny file-write*
  (subpath "/System")
  (subpath "/bin")
  (subpath "/sbin")
  (subpath "/usr/bin")
  (subpath "/usr/sbin")
  (subpath "/usr/lib")
  (subpath "/usr/libexec")
  (subpath "/private/etc")
  (subpath "/Library")
  (subpath "{data_data_str}")
  (literal "{config_str}")
)
(deny file-read*
  (subpath "{data_data_str}")
  (literal "{config_str}")
)"#
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
pub(crate) fn protected_command(program: &str, data_dir: &Path) -> Command {
    if !Path::new(SANDBOX_EXEC).exists() {
        warn!("sandbox-exec not found at {SANDBOX_EXEC}; falling back to code-level protection");
        return Command::new(program);
    }

    let profile = build_profile(data_dir);
    let mut cmd = Command::new(SANDBOX_EXEC);
    cmd.arg("-p").arg(profile).arg("--").arg(program);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_profile_blocks_system_dirs() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile = build_profile(&data_dir);
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
        let profile = build_profile(&data_dir);
        assert!(
            profile.contains("/home/user/.kernex/data"),
            "should block data dir (memory.db)"
        );
    }

    #[test]
    fn test_profile_allows_usr_local() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = build_profile(&data_dir);
        assert!(
            !profile.contains(r#"(subpath "/usr/local")"#),
            "/usr/local should not be blocked"
        );
    }

    #[test]
    fn test_profile_allows_by_default() {
        let data_dir = PathBuf::from("/tmp/ws");
        let profile = build_profile(&data_dir);
        assert!(
            profile.contains("(allow default)"),
            "should allow everything by default"
        );
    }

    #[test]
    fn test_profile_blocks_data_dir_reads() {
        let data_dir = PathBuf::from("/home/user/.kernex");
        let profile = build_profile(&data_dir);
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
        let profile = build_profile(&data_dir);
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
        let profile = build_profile(&data_dir);
        assert!(
            profile.contains(r#"(literal "/home/user/.kernex/config.toml")"#),
            "should block reads to config.toml"
        );
    }

    #[test]
    fn test_command_structure() {
        let data_dir = PathBuf::from("/tmp/ws");
        let cmd = protected_command("claude", &data_dir);
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        assert!(
            program.contains("sandbox-exec") || program.contains("claude"),
            "unexpected program: {program}"
        );
    }
}
