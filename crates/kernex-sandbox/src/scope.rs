//! The directories a sandboxed tool call is scoped to.

use std::path::PathBuf;

/// The host directories that define what a tool call may touch.
///
/// The workspace is the only directory in the scope that tools may write.
/// The data directory holds the runtime's own state (memory database,
/// skills, projects, configuration) and is protected from tool writes; the
/// database and the configuration file are protected from tool reads too.
///
/// Pass the directories the host really uses. A scope built from a guessed
/// data directory protects the guess, not the host's state.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SandboxScope {
    /// The directory tools work in. Writable.
    pub workspace: PathBuf,
    /// The runtime data directory (for example `~/.kernex`, already
    /// expanded). Not writable by tools.
    pub data_dir: PathBuf,
    /// The configuration file, when it lives outside
    /// `{data_dir}/config.toml`. Neither readable nor writable by tools.
    pub config_path: Option<PathBuf>,
    /// The memory database, when it lives outside `{data_dir}/memory.db`.
    /// Neither readable nor writable by tools, together with its `-wal` and
    /// `-shm` files.
    pub db_path: Option<PathBuf>,
}

impl SandboxScope {
    /// Create a scope from the workspace and the runtime data directory.
    pub fn new(workspace: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            data_dir: data_dir.into(),
            config_path: None,
            db_path: None,
        }
    }

    /// Protect a configuration file that lives outside the data directory.
    pub fn with_config_path(mut self, config_path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(config_path.into());
        self
    }

    /// Protect a memory database that lives outside the data directory.
    pub fn with_db_path(mut self, db_path: impl Into<PathBuf>) -> Self {
        self.db_path = Some(db_path.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_builder_sets_optional_paths() {
        let scope = SandboxScope::new("/work/proj", "/state")
            .with_config_path("/etc/agent/config.toml")
            .with_db_path("/var/lib/agent/memory.db");
        assert_eq!(scope.workspace, PathBuf::from("/work/proj"));
        assert_eq!(scope.data_dir, PathBuf::from("/state"));
        assert_eq!(
            scope.config_path,
            Some(PathBuf::from("/etc/agent/config.toml"))
        );
        assert_eq!(
            scope.db_path,
            Some(PathBuf::from("/var/lib/agent/memory.db"))
        );
    }
}
