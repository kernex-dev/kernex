//! Path checks for in-process file tools.
//!
//! A file tool (`read`, `write`, `edit`, `grep`, `glob`) runs inside the host
//! process, so no OS profile stands behind it. Its reach is narrower than a
//! sandboxed subprocess: the workspace, `SandboxProfile::allowed_paths` and,
//! for reads, `SandboxProfile::read_paths` and the runtime's `skills/` and
//! `projects/` directories. The protected paths stay denied inside those
//! roots.
//!
//! Each check returns the path with its symlinks resolved. The tool must open
//! that path, not the one it was given, and open it without following a
//! symlink at the last component ([`no_follow`]): the file it reaches is then
//! the file that was checked.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::policy::Policy;
use crate::{SandboxProfile, SandboxScope};

/// A file tool was refused a path.
///
/// One value covers every reason (outside the tool roots, protected,
/// relative, unusable policy), so the answer does not reveal which rule
/// matched or whether the path exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolPathDenied;

impl fmt::Display for ToolPathDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("path is outside the tool scope or protected")
    }
}

impl std::error::Error for ToolPathDenied {}

/// Resolve `path` for a file tool that reads it.
///
/// Allowed under the workspace, `allowed_paths`, `read_paths` and the data
/// directory's `skills/` and `projects/`, minus the read-deny paths. A
/// workspace that is refused as a write root stays readable. `path` must be
/// absolute.
pub fn tool_read_path(
    path: &Path,
    scope: &SandboxScope,
    profile: &SandboxProfile,
) -> Result<PathBuf, ToolPathDenied> {
    Policy::new(scope, profile)
        .check_tool_read(path)
        .map_err(|_| ToolPathDenied)
}

/// Resolve `path` for a file tool that creates or changes it.
///
/// Allowed under the workspace and `allowed_paths`, minus the write-deny
/// paths. `path` must be absolute; it does not have to exist.
pub fn tool_write_path(
    path: &Path,
    scope: &SandboxScope,
    profile: &SandboxProfile,
) -> Result<PathBuf, ToolPathDenied> {
    Policy::new(scope, profile)
        .check_tool_write(path)
        .map_err(|_| ToolPathDenied)
}

/// The read-deny paths at or below `root`, resolved.
///
/// A search tool that walks `root` leaves these out of its results. A
/// policy that cannot be used is refused, as the path checks refuse it: its
/// deny list may name only part of what it protects.
pub fn tool_search_exclusions(
    root: &Path,
    scope: &SandboxScope,
    profile: &SandboxProfile,
) -> Result<Vec<PathBuf>, ToolPathDenied> {
    let policy = Policy::new(scope, profile);
    if policy.invalid().is_some() {
        return Err(ToolPathDenied);
    }
    Ok(policy
        .read_deny()
        .iter()
        .filter(|denied| denied.starts_with(root))
        .cloned()
        .collect())
}

/// `path` with its symlinks resolved, the way the tool checks resolve it:
/// the deepest existing ancestor is canonicalized and the remaining
/// components are appended. For a host that matches its own rules against
/// the file a tool would really open.
pub fn resolved_path(path: &Path) -> PathBuf {
    crate::policy::resolve(path)
}

/// Make `options` refuse to open a symlink at the last path component.
///
/// On a platform without that flag the options are returned unchanged.
pub fn no_follow(options: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, SandboxScope) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "a").unwrap();
        std::fs::write(root.join("outside/secret"), "s").unwrap();
        std::fs::write(root.join("docs/guide.md"), "g").unwrap();
        let scope = SandboxScope::new(&ws, root.join("state"));
        (dir, root, scope)
    }

    #[test]
    fn test_read_is_confined_to_the_workspace_and_read_paths() {
        let (_dir, root, scope) = fixture();
        let plain = SandboxProfile::default();
        assert!(tool_read_path(&root.join("ws/src/a.rs"), &scope, &plain).is_ok());
        assert_eq!(
            tool_read_path(&root.join("outside/secret"), &scope, &plain),
            Err(ToolPathDenied)
        );
        assert_eq!(
            tool_read_path(Path::new("/etc/hosts"), &scope, &plain),
            Err(ToolPathDenied)
        );
        assert_eq!(
            tool_read_path(Path::new("src/a.rs"), &scope, &plain),
            Err(ToolPathDenied)
        );

        let extra = SandboxProfile::new().with_read_paths(vec![root.join("docs")]);
        assert!(tool_read_path(&root.join("docs/guide.md"), &scope, &extra).is_ok());
        // A read root is not a write root.
        assert_eq!(
            tool_write_path(&root.join("docs/guide.md"), &scope, &extra),
            Err(ToolPathDenied)
        );
    }

    #[test]
    fn test_write_is_confined_to_the_workspace_and_allowed_paths() {
        let (_dir, root, scope) = fixture();
        let plain = SandboxProfile::default();
        assert!(tool_write_path(&root.join("ws/new/file.txt"), &scope, &plain).is_ok());
        assert_eq!(
            tool_write_path(&root.join("outside/new.txt"), &scope, &plain),
            Err(ToolPathDenied)
        );
        // The temp directories are for subprocesses, not for file tools.
        assert_eq!(
            tool_write_path(&std::env::temp_dir().join("kx-tool.txt"), &scope, &plain),
            Err(ToolPathDenied)
        );
        let extra = SandboxProfile::new().with_allowed_paths(vec![root.join("outside")]);
        assert!(tool_write_path(&root.join("outside/new.txt"), &scope, &extra).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_symlinks_that_leave_the_scope_are_refused() {
        let (_dir, root, scope) = fixture();
        let plain = SandboxProfile::default();
        // A link to a file outside, a link to a directory outside, and a
        // dangling link whose target would be created outside.
        std::os::unix::fs::symlink(root.join("outside/secret"), root.join("ws/link")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("ws/dir")).unwrap();
        std::os::unix::fs::symlink(root.join("outside/new"), root.join("ws/dangling")).unwrap();
        for path in ["ws/link", "ws/dir/secret", "ws/dir/new.txt", "ws/dangling"] {
            let path = root.join(path);
            assert_eq!(
                tool_read_path(&path, &scope, &plain),
                Err(ToolPathDenied),
                "{path:?}"
            );
            assert_eq!(
                tool_write_path(&path, &scope, &plain),
                Err(ToolPathDenied),
                "{path:?}"
            );
        }
        // A link that stays inside resolves to its target.
        std::os::unix::fs::symlink(root.join("ws/src/a.rs"), root.join("ws/inner")).unwrap();
        assert_eq!(
            tool_read_path(&root.join("ws/inner"), &scope, &plain),
            Ok(root.join("ws/src/a.rs"))
        );
    }

    #[test]
    fn test_skills_and_a_rejected_workspace_stay_readable() {
        let (_dir, root, scope) = fixture();
        let plain = SandboxProfile::default();
        let skill = root.join("state/skills/review/SKILL.md");
        assert!(tool_read_path(&skill, &scope, &plain).is_ok());
        assert_eq!(tool_write_path(&skill, &scope, &plain), Err(ToolPathDenied));
        assert_eq!(
            tool_read_path(&root.join("state/memory.db"), &scope, &plain),
            Err(ToolPathDenied)
        );

        // The filesystem root is never a write root; reads below it work,
        // minus the protected paths.
        let wide = SandboxScope::new("/", root.join("state"));
        assert!(tool_read_path(&root.join("outside/secret"), &wide, &plain).is_ok());
        assert_eq!(
            tool_write_path(&root.join("outside/secret"), &wide, &plain),
            Err(ToolPathDenied)
        );
        assert_eq!(
            tool_read_path(&root.join("state/memory.db"), &wide, &plain),
            Err(ToolPathDenied)
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_chain_longer_than_the_walk_is_refused() {
        let (_dir, root, scope) = fixture();
        let plain = SandboxProfile::default();
        // m0 -> m1 -> ... -> m39 -> a missing file outside the workspace.
        let last = 39;
        std::os::unix::fs::symlink(
            root.join("outside/missing"),
            root.join(format!("ws/m{last}")),
        )
        .unwrap();
        for i in (0..last).rev() {
            std::os::unix::fs::symlink(
                root.join(format!("ws/m{}", i + 1)),
                root.join(format!("ws/m{i}")),
            )
            .unwrap();
        }
        let head = root.join("ws/m0");
        assert_eq!(tool_read_path(&head, &scope, &plain), Err(ToolPathDenied));
        assert_eq!(tool_write_path(&head, &scope, &plain), Err(ToolPathDenied));
    }

    #[test]
    fn test_protected_paths_stay_denied_inside_the_roots() {
        let (_dir, root, _) = fixture();
        // State inside the workspace, and a blocked directory.
        let scope = SandboxScope::new(root.join("ws"), root.join("ws/.agent"));
        let profile = SandboxProfile::new().with_blocked_paths(vec![root.join("ws/secrets")]);
        for path in ["ws/.agent/memory.db", "ws/secrets/token"] {
            let path = root.join(path);
            assert_eq!(tool_read_path(&path, &scope, &profile), Err(ToolPathDenied));
            assert_eq!(
                tool_write_path(&path, &scope, &profile),
                Err(ToolPathDenied)
            );
        }
        // Skills are readable and not writable.
        let skill = root.join("ws/.agent/skills/x/SKILL.md");
        assert!(tool_read_path(&skill, &scope, &profile).is_ok());
        assert_eq!(
            tool_write_path(&skill, &scope, &profile),
            Err(ToolPathDenied)
        );

        let exclusions = tool_search_exclusions(&root.join("ws"), &scope, &profile).unwrap();
        assert!(exclusions.contains(&root.join("ws/secrets")));
        assert!(exclusions.contains(&root.join("ws/.agent/memory.db")));
        assert!(exclusions.iter().all(|p| p.starts_with(root.join("ws"))));
    }

    #[test]
    fn test_search_exclusions_refuse_an_unusable_policy() {
        let (_dir, root, scope) = fixture();
        // A relative path makes the policy unusable.
        let profile = SandboxProfile::new().with_blocked_paths(vec![PathBuf::from("secrets")]);
        assert_eq!(
            tool_search_exclusions(&root.join("ws"), &scope, &profile),
            Err(ToolPathDenied)
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_no_follow_refuses_a_symlink_leaf() {
        let (_dir, root, _) = fixture();
        std::os::unix::fs::symlink(root.join("outside/secret"), root.join("ws/link")).unwrap();
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        assert!(no_follow(&mut options).open(root.join("ws/link")).is_err());
        assert!(no_follow(&mut options)
            .open(root.join("ws/src/a.rs"))
            .is_ok());
    }
}
