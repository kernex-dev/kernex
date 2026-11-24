//! The path policy behind every sandbox check.
//!
//! One [`Policy`] value is built from a [`SandboxScope`] and a
//! [`SandboxProfile`]. The OS profile generators and the code-level checks
//! ([`crate::is_read_blocked`], [`crate::is_write_blocked`]) all read the
//! same value.
//!
//! The rules:
//!
//! - A write is allowed only under a write root, and never under a
//!   write-deny path. Deny wins over a root that contains it.
//! - A read is allowed everywhere except under a read-deny path.
//! - A read-deny path is write-denied too: a file that could be renamed
//!   would leave its protected name.
//! - `allowed_paths` lifts a default credential entry it names exactly,
//!   never the runtime's state or a `blocked_paths` entry. An entry that
//!   contains the home directory, or that is itself a symlink, is ignored.
//!
//! Every path is compared after symlink resolution, including a path whose
//! last components do not exist yet.
//!
//! A protected path inside a write root could be reached by renaming one of
//! the directories above it, so the policy lists those directories
//! ([`Policy::guards`]) and the OS profile denies writing them. A write root
//! replaced by a symlink, or a link on the way to a root or a protected path
//! repointed, would carry the next sandbox's access elsewhere, so the policy
//! also lists the roots, those links and the directories between them and
//! an enclosing root ([`Policy::pins`]): the OS profile denies creating,
//! renaming or removing them.

use std::path::{Path, PathBuf};

pub(crate) use crate::resolve::resolve;
use crate::resolve::resolve_checked;
use crate::{SandboxProfile, SandboxScope};

/// System locations no tool call may write.
///
/// Device and kernel filesystems (`/dev`, `/proc`, `/sys`) are not listed:
/// they are outside every write root, so the code-level check refuses them,
/// and a subprocess must keep writing `/dev/null` and its terminal.
const SYSTEM_WRITE_DENY: &[&str] = &[
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
];

/// Why a path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denial {
    /// The path is not absolute.
    Relative,
    /// The path is under a protected location.
    Protected,
    /// The path is outside every root that applies to the request.
    OutsideWriteRoots,
    /// The policy could not be built, so nothing is allowed.
    InvalidPolicy,
}

/// Why the workspace was not accepted as a write root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceRejection {
    /// The workspace is the home directory, one of its ancestors, or `/`.
    ContainsHome,
    /// The workspace is inside a location tools may not write.
    Protected,
}

impl WorkspaceRejection {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::ContainsHome => "it is the home directory or one of its ancestors",
            Self::Protected => "it is inside a protected directory",
        }
    }
}

/// The host facts a policy depends on, passed in so tests can pin them.
pub(crate) struct HostDirs {
    pub home: PathBuf,
    pub temp_dirs: Vec<PathBuf>,
}

impl HostDirs {
    /// Read the home directory and the temp directories from the host.
    pub(crate) fn detect() -> Self {
        let temp = std::env::temp_dir();
        let mut temp_dirs = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
        // macOS gives each user `<dir>/T` for temp files and `<dir>/C` for
        // caches; compilers keep their module caches in the second.
        #[cfg(target_os = "macos")]
        if temp.starts_with("/var/folders") && temp.file_name().is_some_and(|n| n == "T") {
            temp_dirs.push(temp.with_file_name("C"));
        }
        temp_dirs.push(temp);
        Self {
            home: crate::resolved_home(),
            temp_dirs,
        }
    }
}

/// Resolved path sets for one scope and profile.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    write_roots: Vec<PathBuf>,
    write_deny: Vec<PathBuf>,
    read_deny: Vec<PathBuf>,
    guards: Vec<PathBuf>,
    pins: Vec<PathBuf>,
    tool_write_roots: Vec<PathBuf>,
    tool_read_roots: Vec<PathBuf>,
    workspace_rejection: Option<WorkspaceRejection>,
    invalid: Option<String>,
}

impl Policy {
    /// Build the policy for this host.
    pub(crate) fn new(scope: &SandboxScope, profile: &SandboxProfile) -> Self {
        Self::with_host(scope, profile, &HostDirs::detect())
    }

    /// Build the policy against explicit host directories.
    pub(crate) fn with_host(
        scope: &SandboxScope,
        profile: &SandboxProfile,
        host: &HostDirs,
    ) -> Self {
        let mut invalid = first_relative(scope, profile);
        let home = resolve(&host.home);
        let workspace = resolve(&scope.workspace);

        // Locations holding the runtime's own state. Both lists are prefixes:
        // a directory entry covers everything below it.
        let mut state: Vec<PathBuf> = vec![scope.data_dir.join("data")];
        push_database(&mut state, &scope.data_dir.join("memory.db"));
        if let Some(db) = &scope.db_path {
            push_database(&mut state, db);
        }
        state.push(scope.data_dir.join("config.toml"));
        if let Some(config) = &scope.config_path {
            state.push(config.clone());
        }
        let blocked = profile.blocked_paths.iter();

        let mut write_deny: Vec<PathBuf> = SYSTEM_WRITE_DENY.iter().map(PathBuf::from).collect();
        let mut read_deny: Vec<PathBuf> = Vec::new();
        for path in state.iter().chain(blocked) {
            push_unique(&mut write_deny, resolve(path));
            push_unique(&mut read_deny, resolve(path));
        }
        for dir in crate::paths::DATA_READ_ONLY {
            push_unique(&mut write_deny, resolve(&scope.data_dir.join(dir)));
        }
        let allowed: Vec<PathBuf> = profile
            .allowed_paths
            .iter()
            .filter(|p| p.is_absolute())
            // A link put in place of the entry would carry the grant elsewhere.
            .filter(|p| !p.symlink_metadata().is_ok_and(|m| m.is_symlink()))
            .map(|p| resolve(p))
            // Like a misplaced temp dir: it would make the whole home writable.
            .filter(|p| !home.starts_with(p))
            .collect();
        for cred in crate::credential_read_deny_dirs(&home) {
            let cred = resolve(&cred);
            // A host that hands a credential location to the subprocess
            // (an agent CLI's own state) has decided it may use it.
            if !allowed.contains(&cred) {
                push_unique(&mut write_deny, cred.clone());
                push_unique(&mut read_deny, cred);
            }
        }
        for dir in crate::paths::AGENT_CLI_STATE {
            let dir = resolve(&scope.data_dir.join(dir));
            if !allowed.contains(&dir) {
                push_unique(&mut write_deny, dir.clone());
                push_unique(&mut read_deny, dir);
            }
        }
        for dir in crate::paths::toolchain_exec_paths(&home) {
            push_unique(&mut write_deny, resolve(&dir));
        }

        let workspace_rejection = if home.starts_with(&workspace) {
            Some(WorkspaceRejection::ContainsHome)
        } else if under_any_deny(&workspace, &write_deny) {
            Some(WorkspaceRejection::Protected)
        } else {
            None
        };

        let mut write_roots: Vec<PathBuf> = Vec::new();
        if workspace_rejection.is_none() && scope.workspace.is_absolute() {
            push_unique(&mut write_roots, workspace);
        }
        // In-process file tools: the workspace and the host's explicit
        // paths only, not the temp and toolchain directories.
        let mut tool_write_roots = write_roots.clone();
        for dir in &allowed {
            push_unique(&mut tool_write_roots, dir.clone());
        }
        let mut tool_read_roots = tool_write_roots.clone();
        // A workspace that is refused as a write root (the home directory,
        // say) can still be read, and so can the skills and projects the
        // runtime points the model at.
        if scope.workspace.is_absolute() {
            push_unique(&mut tool_read_roots, resolve(&scope.workspace));
        }
        for dir in ["skills", "projects"] {
            push_unique(&mut tool_read_roots, resolve(&scope.data_dir.join(dir)));
        }
        for dir in profile.read_paths.iter().filter(|p| p.is_absolute()) {
            push_unique(&mut tool_read_roots, resolve(dir));
        }
        for dir in &host.temp_dirs {
            // A temp dir that contains the home directory (a misplaced
            // `TMPDIR`) would make the whole home writable.
            let dir = resolve(dir);
            if dir.is_absolute() && !home.starts_with(&dir) {
                push_unique(&mut write_roots, dir);
            }
        }
        for dir in crate::write_allow_dirs(&home) {
            push_unique(&mut write_roots, resolve(&dir));
        }
        for dir in allowed {
            push_unique(&mut write_roots, dir);
        }

        // A link on the way to a root or a protected path, inside a root,
        // could be repointed, so every path the policy is built from is
        // walked for links.
        let data = crate::paths::DATA_READ_ONLY
            .iter()
            .chain(crate::paths::AGENT_CLI_STATE);
        let mut named: Vec<PathBuf> = data.map(|d| scope.data_dir.join(d)).collect();
        named.extend(crate::credential_read_deny_dirs(&home));
        named.extend(crate::paths::toolchain_exec_paths(&home));
        named.extend(crate::write_allow_dirs(&home));
        named.extend([scope.workspace.clone(), scope.data_dir.clone()]);
        let given = host.temp_dirs.iter().chain(&profile.allowed_paths);
        named.extend(given.chain(&profile.blocked_paths).chain(&state).cloned());
        let denied = write_deny.iter().chain(&read_deny).map(PathBuf::as_path);
        let (guards, pins, too_many) = crate::guards::guards_and_pins(&write_roots, denied, &named);
        invalid = invalid.or(too_many);

        Self {
            write_roots,
            write_deny,
            read_deny,
            guards,
            pins,
            tool_write_roots,
            tool_read_roots,
            workspace_rejection,
            invalid,
        }
    }

    /// Directories a tool call may write under.
    pub(crate) fn write_roots(&self) -> &[PathBuf] {
        &self.write_roots
    }

    /// Paths no tool call may write, even inside a write root.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    pub(crate) fn write_deny(&self) -> &[PathBuf] {
        &self.write_deny
    }

    /// Paths no tool call may read.
    pub(crate) fn read_deny(&self) -> &[PathBuf] {
        &self.read_deny
    }

    /// Directories above a protected path that sit inside a write root.
    /// Renaming or removing one of them would move the protected path, and
    /// a change of mode or ACL would let another account do it, so the OS
    /// profile denies writes to these directories themselves (not to their
    /// other contents). Read by the Seatbelt generator only.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn guards(&self) -> &[PathBuf] {
        &self.guards
    }

    /// The directory write roots (a file root's directory), the symlinks
    /// met on the way to a root or a protected path and the directories a
    /// `..` steps out of there, and the directories between them and an
    /// enclosing write root. A policy whose paths go through more symlinks
    /// than can be followed is invalid. Removing, renaming or replacing one of them would change
    /// what the next policy resolves, so the OS profile denies creating and
    /// removing these paths; their contents, timestamps and mode stay
    /// writable. A write root that does not exist yet cannot be created
    /// inside the sandbox. Read by the Seatbelt generator only.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn pins(&self) -> &[PathBuf] {
        &self.pins
    }

    /// Set when the policy could not be built from its inputs. Nothing is
    /// allowed under an invalid policy.
    pub(crate) fn invalid(&self) -> Option<&str> {
        self.invalid.as_deref()
    }

    /// Set when the workspace was not accepted as a write root.
    pub(crate) fn workspace_rejection(&self) -> Option<WorkspaceRejection> {
        self.workspace_rejection
    }

    /// Decide whether `path` may be read.
    pub(crate) fn check_read(&self, path: &Path) -> Result<(), Denial> {
        if !path.is_absolute() {
            return Err(Denial::Relative);
        }
        if self.invalid.is_some() {
            return Err(Denial::InvalidPolicy);
        }
        // A path that cannot be resolved completely is refused: what the
        // kernel opens through it is unknown.
        let resolved = resolve_checked(path).map_err(|_| Denial::Protected)?;
        if under_any_deny(&resolved, &self.read_deny) {
            return Err(Denial::Protected);
        }
        Ok(())
    }

    /// Decide whether an in-process file tool may read `path`, and return
    /// the resolved path it must open.
    pub(crate) fn check_tool_read(&self, path: &Path) -> Result<PathBuf, Denial> {
        self.check_tool(path, &self.tool_read_roots, &self.read_deny)
    }

    /// Decide whether an in-process file tool may write `path`, and return
    /// the resolved path it must open.
    pub(crate) fn check_tool_write(&self, path: &Path) -> Result<PathBuf, Denial> {
        self.check_tool(path, &self.tool_write_roots, &self.write_deny)
    }

    fn check_tool(
        &self,
        path: &Path,
        roots: &[PathBuf],
        deny: &[PathBuf],
    ) -> Result<PathBuf, Denial> {
        if !path.is_absolute() {
            return Err(Denial::Relative);
        }
        if self.invalid.is_some() {
            return Err(Denial::InvalidPolicy);
        }
        let resolved = resolve_checked(path).map_err(|_| Denial::Protected)?;
        if under_any_deny(&resolved, deny) {
            return Err(Denial::Protected);
        }
        // Still a symlink after resolution: a chain longer than the walk
        // follows. Where it ends is unknown, so it is refused.
        if resolved
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err(Denial::Protected);
        }
        if under_any(&resolved, roots) {
            return Ok(resolved);
        }
        Err(Denial::OutsideWriteRoots)
    }

    /// Decide whether `path` may be written.
    pub(crate) fn check_write(&self, path: &Path) -> Result<(), Denial> {
        if !path.is_absolute() {
            return Err(Denial::Relative);
        }
        if self.invalid.is_some() {
            return Err(Denial::InvalidPolicy);
        }
        let resolved = resolve_checked(path).map_err(|_| Denial::Protected)?;
        if under_any_deny(&resolved, &self.write_deny) {
            return Err(Denial::Protected);
        }
        if under_any(&resolved, &self.write_roots) {
            return Ok(());
        }
        Err(Denial::OutsideWriteRoots)
    }
}

/// A SQLite database and the journal files it creates beside itself.
fn push_database(list: &mut Vec<PathBuf>, db: &Path) {
    list.push(db.to_path_buf());
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = db.as_os_str().to_os_string();
        name.push(suffix);
        list.push(PathBuf::from(name));
    }
}

pub(crate) fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

fn under_any(path: &Path, prefixes: &[PathBuf]) -> bool {
    prefixes.iter().any(|prefix| path.starts_with(prefix))
}

/// Like [`under_any`], for deny lists. macOS volumes are case-insensitive
/// by default and a component that does not exist yet keeps the caller's
/// spelling, so there a deny entry matches whatever the case.
#[cfg(target_os = "macos")]
fn under_any_deny(path: &Path, prefixes: &[PathBuf]) -> bool {
    prefixes.iter().any(|prefix| {
        let mut have = path.components();
        prefix.components().all(|want| {
            have.next()
                .is_some_and(|got| got.as_os_str().eq_ignore_ascii_case(want.as_os_str()))
        })
    })
}

#[cfg(not(target_os = "macos"))]
fn under_any_deny(path: &Path, prefixes: &[PathBuf]) -> bool {
    under_any(path, prefixes)
}

/// The first scope or `blocked_paths` entry that is not absolute, if any.
/// Such a path cannot be resolved and therefore cannot be protected.
fn first_relative(scope: &SandboxScope, profile: &SandboxProfile) -> Option<String> {
    let named = [
        ("workspace", Some(&scope.workspace)),
        ("data_dir", Some(&scope.data_dir)),
        ("config_path", scope.config_path.as_ref()),
        ("db_path", scope.db_path.as_ref()),
    ];
    let blocked = profile
        .blocked_paths
        .iter()
        .map(|p| ("blocked_paths entry", Some(p)));
    named
        .into_iter()
        .chain(blocked)
        .filter_map(|(what, path)| path.map(|p| (what, p)))
        .find(|(_, path)| !path.is_absolute())
        .map(|(what, path)| format!("{what} is not an absolute path: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic absolute paths that exist on no host, so resolution leaves
    // them as written and the assertions do not depend on the machine.
    const HOME: &str = "/kx-policy-test/home/ana";

    fn host() -> HostDirs {
        HostDirs {
            home: PathBuf::from(HOME),
            temp_dirs: vec![PathBuf::from("/kx-policy-test/tmp")],
        }
    }

    fn scope() -> SandboxScope {
        SandboxScope::new(format!("{HOME}/code/proj"), format!("{HOME}/.kernex"))
    }

    fn policy() -> Policy {
        Policy::with_host(&scope(), &SandboxProfile::default(), &host())
    }

    fn p(suffix: &str) -> PathBuf {
        PathBuf::from(format!("{HOME}/{suffix}"))
    }

    #[test]
    fn test_workspace_is_writable() {
        let policy = policy();
        assert_eq!(policy.check_write(&p("code/proj/src/main.rs")), Ok(()));
        assert_eq!(policy.check_write(&p("code/proj")), Ok(()));
    }

    #[test]
    fn test_sibling_of_workspace_is_not_writable() {
        let policy = policy();
        assert_eq!(
            policy.check_write(&p("code/other/src/main.rs")),
            Err(Denial::OutsideWriteRoots)
        );
        assert_eq!(
            policy.check_write(&p("code/proj-backup/file")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[test]
    fn test_home_dotfile_is_not_writable() {
        assert_eq!(
            policy().check_write(&p(".zshrc")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[test]
    fn test_data_dir_is_not_a_write_root() {
        let policy = policy();
        assert_eq!(
            policy.check_write(&p(".kernex/notes.txt")),
            Err(Denial::OutsideWriteRoots)
        );
        assert!(!policy.write_roots().contains(&p(".kernex")));
    }

    #[test]
    fn test_installed_programs_under_local_share_are_not_writable() {
        for path in [
            ".local/share/claude/versions/9.9.9",
            ".local/share/uv/tools/x/bin/y",
            ".local/share/pipx/venvs/x/bin/y",
        ] {
            assert_eq!(
                policy().check_write(&p(path)),
                Err(Denial::OutsideWriteRoots),
                "{path}"
            );
        }
    }

    #[test]
    fn test_agent_cli_state_is_open_only_to_its_cli() {
        // The data dir inside a write root, so only the deny entries hold.
        let host = HostDirs {
            temp_dirs: vec![p("tmp")],
            ..host()
        };
        let scope = SandboxScope::new(p("code/proj"), p("tmp/state"));
        let others = Policy::with_host(&scope, &SandboxProfile::default(), &host);
        for dir in ["cli/claude", "cli/opencode"] {
            let file = p(&format!("tmp/state/{dir}/f"));
            assert_eq!(others.check_write(&file), Err(Denial::Protected), "{dir}");
            assert_eq!(others.check_read(&file), Err(Denial::Protected), "{dir}");
        }
        for dir in ["cli/mcp-config", "mcp"] {
            let file = p(&format!("tmp/state/{dir}/f"));
            assert_eq!(others.check_write(&file), Err(Denial::Protected), "{dir}");
            assert_eq!(others.check_read(&file), Ok(()), "{dir}");
        }

        let claude = SandboxProfile::new().with_allowed_paths(vec![p("tmp/state/cli/claude")]);
        let claude = Policy::with_host(&scope, &claude, &host);
        assert_eq!(claude.check_write(&p("tmp/state/cli/claude/f")), Ok(()));
        assert_eq!(
            claude.check_write(&p("tmp/state/cli/opencode/run-1/f")),
            Err(Denial::Protected)
        );
        // Naming a directory above the state lifts nothing.
        let above = SandboxProfile::new().with_allowed_paths(vec![p("tmp/state/cli")]);
        let above = Policy::with_host(&scope, &above, &host);
        assert_eq!(
            above.check_write(&p("tmp/state/cli/claude/f")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_runtime_state_is_write_denied() {
        let policy = policy();
        for path in [
            ".kernex/memory.db",
            ".kernex/memory.db-wal",
            ".kernex/memory.db-shm",
            ".kernex/data/memory.db",
            ".kernex/config.toml",
            ".kernex/skills/review/SKILL.md",
            ".kernex/projects/site/ROLE.md",
        ] {
            assert_eq!(
                policy.check_write(&p(path)),
                Err(Denial::Protected),
                "{path}"
            );
        }
    }

    #[test]
    fn test_state_inside_the_workspace_stays_denied() {
        // Deny wins over a write root that contains it.
        let scope = SandboxScope::new(p("code/proj"), p("code/proj/.agent"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host());
        assert_eq!(policy.check_write(&p("code/proj/src/lib.rs")), Ok(()));
        assert_eq!(
            policy.check_write(&p("code/proj/.agent/memory.db")),
            Err(Denial::Protected)
        );
        assert_eq!(
            policy.check_write(&p("code/proj/.agent/skills/x/SKILL.md")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_custom_database_and_config_are_protected() {
        let scope = scope()
            .with_db_path("/kx-policy-test/var/agent.db")
            .with_config_path("/kx-policy-test/etc/agent.toml");
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host());
        for path in [
            "/kx-policy-test/var/agent.db",
            "/kx-policy-test/var/agent.db-wal",
            "/kx-policy-test/var/agent.db-shm",
            "/kx-policy-test/etc/agent.toml",
        ] {
            let path = Path::new(path);
            assert_eq!(policy.check_write(path), Err(Denial::Protected));
            assert_eq!(policy.check_read(path), Err(Denial::Protected));
        }
        assert_eq!(
            policy.check_read(Path::new("/kx-policy-test/etc/other.toml")),
            Ok(())
        );
    }

    #[test]
    fn test_skills_are_readable_and_state_is_not() {
        let policy = policy();
        assert_eq!(
            policy.check_read(&p(".kernex/skills/review/SKILL.md")),
            Ok(())
        );
        assert_eq!(
            policy.check_read(&p(".kernex/projects/site/ROLE.md")),
            Ok(())
        );
        assert_eq!(
            policy.check_read(&p(".kernex/memory.db")),
            Err(Denial::Protected)
        );
        assert_eq!(
            policy.check_read(&p(".kernex/data/memory.db")),
            Err(Denial::Protected)
        );
        assert_eq!(
            policy.check_read(&p(".kernex/config.toml")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_credentials_are_not_readable() {
        let policy = policy();
        assert_eq!(
            policy.check_read(&p(".ssh/id_ed25519")),
            Err(Denial::Protected)
        );
        assert_eq!(
            policy.check_read(&p(".aws/credentials")),
            Err(Denial::Protected)
        );
        assert_eq!(policy.check_read(&p("code/other/README.md")), Ok(()));
    }

    #[test]
    fn test_temp_and_toolchain_dirs_are_writable() {
        let policy = policy();
        assert_eq!(
            policy.check_write(Path::new("/kx-policy-test/tmp/build/out")),
            Ok(())
        );
        assert_eq!(policy.check_write(&p(".cargo/registry/cache/x")), Ok(()));
    }

    #[test]
    fn test_system_dirs_are_write_denied() {
        let policy = policy();
        for path in ["/bin/sh", "/usr/bin/env", "/etc/hosts", "/System/Library/x"] {
            assert_eq!(
                policy.check_write(Path::new(path)),
                Err(Denial::Protected),
                "{path}"
            );
        }
        // Prefixes match whole components.
        assert_eq!(
            policy.check_write(Path::new("/binaries/x")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[test]
    fn test_relative_paths_are_refused() {
        let policy = policy();
        assert_eq!(
            policy.check_write(Path::new("src/main.rs")),
            Err(Denial::Relative)
        );
        assert_eq!(
            policy.check_read(Path::new("../../.ssh/id_rsa")),
            Err(Denial::Relative)
        );
    }

    #[test]
    fn test_parent_components_cannot_leave_the_workspace() {
        let policy = policy();
        assert_eq!(
            policy.check_write(&p("code/proj/../other/file")),
            Err(Denial::OutsideWriteRoots)
        );
        assert_eq!(
            policy.check_read(&p("code/proj/../../.ssh/id_rsa")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_allowed_path_lifts_only_the_credential_entry_it_names() {
        let read = |allowed: &str| {
            let profile = SandboxProfile::new().with_allowed_paths(vec![p(allowed)]);
            Policy::with_host(&scope(), &profile, &host())
        };
        // Named exactly: readable and writable.
        let named = read(".claude");
        assert_eq!(named.check_read(&p(".claude/settings.json")), Ok(()));
        assert_eq!(named.check_write(&p(".claude/settings.json")), Ok(()));
        assert_eq!(named.check_read(&p(".ssh/id_rsa")), Err(Denial::Protected));
        // A directory above credential entries lifts none of them.
        let above = read(".config");
        assert_eq!(above.check_write(&p(".config/tool/state")), Ok(()));
        assert_eq!(
            above.check_read(&p(".config/gh/hosts.yml")),
            Err(Denial::Protected)
        );
        assert_eq!(
            above.check_write(&p(".config/gh/hosts.yml")),
            Err(Denial::Protected)
        );
        // The home directory or an ancestor is not accepted at all.
        for all in [PathBuf::from(HOME), PathBuf::from("/")] {
            let profile = SandboxProfile::new().with_allowed_paths(vec![all]);
            let policy = Policy::with_host(&scope(), &profile, &host());
            assert_eq!(policy.check_read(&p(".ssh/id_rsa")), Err(Denial::Protected));
            assert_eq!(
                policy.check_write(&p("notes.txt")),
                Err(Denial::OutsideWriteRoots)
            );
        }
        // Runtime state is never lifted.
        let state = read(".kernex");
        assert_eq!(
            state.check_read(&p(".kernex/memory.db")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_credentials_inside_a_write_root_are_write_denied() {
        let policy = policy();
        assert_eq!(policy.check_write(&p(".cargo/registry/x")), Ok(()));
        for path in [".cargo/credentials.toml", ".cargo/env", ".cargo/bin/cargo"] {
            assert_eq!(
                policy.check_write(&p(path)),
                Err(Denial::Protected),
                "{path}"
            );
        }
        assert_eq!(
            policy.check_write(&p(".rustup/toolchains/x")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[test]
    fn test_profile_paths_extend_both_lists() {
        let profile = SandboxProfile::new()
            .with_allowed_paths(vec![PathBuf::from("/kx-policy-test/extra")])
            .with_blocked_paths(vec![p("code/proj/secrets")]);
        let policy = Policy::with_host(&scope(), &profile, &host());
        assert_eq!(
            policy.check_write(Path::new("/kx-policy-test/extra/out.txt")),
            Ok(())
        );
        assert_eq!(
            policy.check_write(&p("code/proj/secrets/key")),
            Err(Denial::Protected)
        );
        assert_eq!(
            policy.check_read(&p("code/proj/secrets/key")),
            Err(Denial::Protected)
        );
    }

    #[test]
    fn test_workspace_at_or_above_home_is_rejected() {
        for workspace in [HOME, "/kx-policy-test/home", "/kx-policy-test", "/"] {
            let scope = SandboxScope::new(workspace, format!("{HOME}/.kernex"));
            let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host());
            assert_eq!(
                policy.workspace_rejection(),
                Some(WorkspaceRejection::ContainsHome),
                "{workspace}"
            );
            assert_eq!(
                policy.check_write(&p("notes.txt")),
                Err(Denial::OutsideWriteRoots),
                "{workspace}"
            );
            // Writes fall back to the temp dirs.
            assert_eq!(
                policy.check_write(Path::new("/kx-policy-test/tmp/x")),
                Ok(()),
                "{workspace}"
            );
        }
    }

    #[test]
    fn test_workspace_inside_protected_dir_is_rejected() {
        let scope = SandboxScope::new(p(".kernex/skills/mine"), p(".kernex"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host());
        assert_eq!(
            policy.workspace_rejection(),
            Some(WorkspaceRejection::Protected)
        );
    }

    #[test]
    fn test_device_files_are_not_write_denied_and_not_writable_in_process() {
        let policy = policy();
        assert!(!policy.write_deny().contains(&PathBuf::from("/dev")));
        assert_eq!(
            policy.check_write(Path::new("/dev/disk0")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[test]
    fn test_relative_scope_or_blocked_path_invalidates_the_policy() {
        let cases = [
            (
                SandboxScope::new("code/proj", "/state"),
                SandboxProfile::default(),
            ),
            (
                SandboxScope::new("/work", "~/.kernex"),
                SandboxProfile::default(),
            ),
            (
                SandboxScope::new("/work", "/state").with_db_path("memory.db"),
                SandboxProfile::default(),
            ),
            (
                SandboxScope::new("/work", "/state"),
                SandboxProfile::new().with_blocked_paths(vec![PathBuf::from("secrets")]),
            ),
        ];
        for (scope, profile) in cases {
            let policy = Policy::with_host(&scope, &profile, &host());
            assert!(policy.invalid().is_some(), "{scope:?}");
            assert_eq!(
                policy.check_read(Path::new("/kx-policy-test/any")),
                Err(Denial::InvalidPolicy)
            );
            assert_eq!(
                policy.check_write(Path::new("/kx-policy-test/tmp/any")),
                Err(Denial::InvalidPolicy)
            );
        }
        assert_eq!(policy().invalid(), None);
    }

    #[test]
    fn test_guards_cover_directories_above_state_inside_a_root() {
        let scope = SandboxScope::new(p("code/proj"), p("code/proj/.agent"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host());
        assert!(policy.guards().contains(&p("code/proj")));
        assert!(policy.guards().contains(&p("code/proj/.agent")));
        assert!(!policy.guards().contains(&p("code")));

        // State outside every root needs no guard. What remains guards the
        // toolchain directories that hold a read-only `bin` or a credential
        // file.
        let default_guards = self::policy().guards().to_vec();
        assert!(default_guards.contains(&p(".cargo")));
        assert!(!default_guards.iter().any(|g| g.starts_with(p(".kernex"))));
        assert!(!default_guards.contains(&p("code")));
    }

    #[test]
    fn test_pins_keep_every_write_root_in_place() {
        // A workspace inside the temp dir: the workspace, and every
        // directory between it and the temp dir, cannot be swapped.
        let scope = SandboxScope::new("/kx-policy-test/tmp/run/ws", format!("{HOME}/.kernex"));
        let profile = SandboxProfile {
            allowed_paths: vec![p(".kernex/cli/claude")],
            ..Default::default()
        };
        let policy = Policy::with_host(&scope, &profile, &host());
        let guards = policy.pins();
        for dir in [
            "/kx-policy-test/tmp/run/ws",
            "/kx-policy-test/tmp/run",
            "/kx-policy-test/tmp",
        ] {
            assert!(guards.contains(&PathBuf::from(dir)), "{dir}");
        }
        assert!(guards.contains(&p(".kernex/cli/claude")));
        assert!(guards.contains(&p(".npm")));
        assert!(!guards.contains(&PathBuf::from("/kx-policy-test")));
        assert!(!guards.contains(&p(".kernex/cli")));
    }

    #[cfg(unix)]
    #[test]
    fn test_a_link_on_the_way_to_a_root_inside_a_root_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = resolve(dir.path());
        std::fs::create_dir_all(tmp.join("real/ws")).unwrap();
        std::os::unix::fs::symlink(tmp.join("real"), tmp.join("link")).unwrap();
        let host = HostDirs {
            home: PathBuf::from(HOME),
            temp_dirs: vec![tmp.clone()],
        };
        let scope = SandboxScope::new(tmp.join("link/ws"), format!("{HOME}/.kernex"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host);
        assert!(policy.write_roots().contains(&tmp.join("real/ws")));
        for guarded in [tmp.join("link"), tmp.join("real/ws"), tmp.join("real")] {
            assert!(policy.pins().contains(&guarded), "{}", guarded.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_the_directory_a_dotdot_leaves_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = resolve(dir.path());
        std::fs::create_dir_all(tmp.join("a/b")).unwrap();
        std::fs::create_dir_all(tmp.join("ws")).unwrap();
        std::os::unix::fs::symlink(tmp.join("a/b"), tmp.join("ws/l")).unwrap();
        let host = HostDirs {
            home: PathBuf::from(HOME),
            temp_dirs: vec![tmp.clone()],
        };
        let scope = SandboxScope::new(tmp.join("ws"), format!("{HOME}/.kernex"));
        let mut profile = SandboxProfile::default();
        profile.allowed_paths.push(tmp.join("ws/l/../state"));
        let policy = Policy::with_host(&scope, &profile, &host);
        assert!(policy.write_roots().contains(&tmp.join("a/state")));
        for pinned in [tmp.join("ws/l"), tmp.join("a/b"), tmp.join("a")] {
            assert!(policy.pins().contains(&pinned), "{}", pinned.display());
        }
        // Pins stay pins: the workspace above them keeps its metadata.
        assert!(!policy.guards().contains(&tmp.join("ws")));
    }

    #[cfg(unix)]
    #[test]
    fn test_a_path_that_cannot_be_resolved_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ws = resolve(dir.path());
        std::fs::create_dir(ws.join("secret")).unwrap();
        std::fs::write(ws.join("secret/key"), "x").unwrap();
        std::os::unix::fs::symlink("loop", ws.join("loop")).unwrap();
        std::os::unix::fs::symlink("loop/..", ws.join("x")).unwrap();
        let scope = SandboxScope::new(&ws, format!("{HOME}/.kernex"));
        let mut profile = SandboxProfile::default();
        profile.blocked_paths.push(ws.join("secret"));
        let policy = Policy::with_host(&scope, &profile, &host());
        for path in ["loop/../secret/key", "x/secret/key", "x/new"] {
            let path = ws.join(path);
            assert_eq!(policy.check_read(&path), Err(Denial::Protected));
            assert_eq!(policy.check_write(&path), Err(Denial::Protected));
            assert_eq!(policy.check_tool_read(&path), Err(Denial::Protected));
            assert_eq!(policy.check_tool_write(&path), Err(Denial::Protected));
        }
        assert!(policy.check_tool_write(&ws.join("new")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_a_dotdot_after_a_missing_component_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let ws = resolve(dir.path());
        std::fs::create_dir(ws.join("secret")).unwrap();
        let scope = SandboxScope::new(&ws, format!("{HOME}/.kernex"));
        let mut profile = SandboxProfile::default();
        profile.blocked_paths.push(ws.join("missing/../secret"));
        let policy = Policy::with_host(&scope, &profile, &host());
        assert!(policy.pins().contains(&ws.join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn test_an_allowed_path_that_is_a_symlink_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        let link = dir.path().join("state");
        std::fs::create_dir(&victim).unwrap();
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let profile = SandboxProfile {
            allowed_paths: vec![link.clone(), dir.path().join("plain")],
            ..Default::default()
        };
        let policy = Policy::with_host(&scope(), &profile, &host());
        let victim = resolve(&victim);
        assert!(!policy.write_roots().iter().any(|r| r == &victim));
        assert_eq!(
            policy.check_tool_write(&link.join("f")),
            Err(Denial::OutsideWriteRoots)
        );
        assert!(policy
            .write_roots()
            .contains(&resolve(&dir.path().join("plain"))));
    }

    #[test]
    fn test_temp_dir_containing_home_is_not_a_write_root() {
        let host = HostDirs {
            home: PathBuf::from(HOME),
            temp_dirs: vec![
                PathBuf::from("/kx-policy-test/home"),
                PathBuf::from(HOME),
                PathBuf::from("/kx-policy-test/tmp"),
            ],
        };
        let policy = Policy::with_host(&scope(), &SandboxProfile::default(), &host);
        assert_eq!(
            policy.check_write(&p(".zshrc")),
            Err(Denial::OutsideWriteRoots)
        );
        assert_eq!(
            policy.check_write(Path::new("/kx-policy-test/tmp/x")),
            Ok(())
        );
    }

    #[test]
    fn test_rollback_journal_is_protected() {
        assert_eq!(
            policy().check_write(&p(".kernex/memory.db-journal")),
            Err(Denial::Protected)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_deny_lists_ignore_case_on_macos() {
        let policy = policy();
        assert_eq!(policy.check_read(&p(".SSH/id_rsa")), Err(Denial::Protected));
        assert_eq!(
            policy.check_read(&p(".kernex/MEMORY.DB")),
            Err(Denial::Protected)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_data_volume_spelling_is_the_same_path() {
        assert_eq!(
            resolve(Path::new("/System/Volumes/Data/kx-policy-test/x")),
            PathBuf::from("/kx-policy-test/x")
        );
        let policy = policy();
        let via_volume = PathBuf::from(format!("/System/Volumes/Data{HOME}/.ssh/id_rsa"));
        assert_eq!(policy.check_read(&via_volume), Err(Denial::Protected));
        let state = PathBuf::from(format!("/System/Volumes/Data{HOME}/.kernex/memory.db"));
        assert_eq!(policy.check_read(&state), Err(Denial::Protected));
    }

    #[test]
    fn test_workspace_below_home_is_accepted() {
        assert_eq!(policy().workspace_rejection(), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlinked_parent_of_a_new_file_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let workspace = root.join("ws");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();

        assert_eq!(
            resolve(&workspace.join("link/new.txt")),
            outside.join("new.txt")
        );

        let host = HostDirs {
            home: root.join("home"),
            temp_dirs: Vec::new(),
        };
        let scope = SandboxScope::new(&workspace, root.join("state"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host);
        assert_eq!(policy.check_write(&workspace.join("real.txt")), Ok(()));
        assert_eq!(
            policy.check_write(&workspace.join("link/new.txt")),
            Err(Denial::OutsideWriteRoots)
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_dangling_symlink_is_followed_to_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let workspace = root.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        let target = root.join("state/memory.db");
        std::os::unix::fs::symlink(&target, workspace.join("db")).unwrap();

        assert_eq!(resolve(&workspace.join("db")), target);

        let host = HostDirs {
            home: root.join("home"),
            temp_dirs: Vec::new(),
        };
        let scope = SandboxScope::new(&workspace, root.join("state"));
        let policy = Policy::with_host(&scope, &SandboxProfile::default(), &host);
        assert_eq!(
            policy.check_write(&workspace.join("db")),
            Err(Denial::Protected)
        );
    }
}
