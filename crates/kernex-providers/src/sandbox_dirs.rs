//! The runtime directories a provider's tool calls must not change.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use kernex_sandbox::SandboxScope;

static DEFAULT_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Environment variable naming the runtime data directory.
const DATA_DIR_ENV: &str = "KERNEX_DATA_DIR";

/// The host's runtime directories, handed to the sandbox so tool calls
/// cannot write the runtime's own state.
///
/// Set `data_dir` to the directory the host really uses. When it is left
/// unset the sandbox protects the runtime's default location
/// (`$KERNEX_DATA_DIR`, or `~/.kernex`), which is only right for a host that
/// keeps the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SandboxDirs {
    /// The runtime data directory, already expanded.
    pub data_dir: Option<PathBuf>,
    /// The configuration file, when it lives outside the data directory.
    pub config_path: Option<PathBuf>,
    /// The memory database, when it lives outside the data directory.
    pub db_path: Option<PathBuf>,
}

impl SandboxDirs {
    /// No directory set: the sandbox falls back to the default data directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the runtime data directory.
    pub fn with_data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(data_dir.into());
        self
    }

    /// Set a configuration file that lives outside the data directory.
    pub fn with_config_path(mut self, config_path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(config_path.into());
        self
    }

    /// Set a memory database that lives outside the data directory.
    pub fn with_db_path(mut self, db_path: impl Into<PathBuf>) -> Self {
        self.db_path = Some(db_path.into());
        self
    }

    /// Read the directories from a provider configuration.
    pub(crate) fn from_config(config: &crate::factory::ProviderConfig) -> Self {
        Self {
            data_dir: config.data_dir.clone(),
            config_path: config.config_path.clone(),
            db_path: config.db_path.clone(),
        }
    }

    /// Build the sandbox scope for a tool call working in `workspace`.
    ///
    /// Warns once per process when no data directory was configured: the
    /// sandbox then protects the default location, which is wrong for a host
    /// that keeps its state elsewhere.
    pub(crate) fn scope(&self, workspace: &Path) -> SandboxScope {
        if self.data_dir.is_none() && !DEFAULT_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                data_dir = %default_data_dir().display(),
                "no data_dir configured for the tool sandbox; protecting the default \
                 location. Set ProviderConfig::data_dir to the directory the host uses"
            );
        }
        self.build_scope(workspace)
    }

    /// [`scope`](Self::scope) without the missing-data-dir warning.
    pub(crate) fn build_scope(&self, workspace: &Path) -> SandboxScope {
        let data_dir = self.data_dir.clone().unwrap_or_else(default_data_dir);
        let mut scope = SandboxScope::new(workspace, data_dir);
        if let Some(config_path) = &self.config_path {
            scope = scope.with_config_path(config_path);
        }
        if let Some(db_path) = &self.db_path {
            scope = scope.with_db_path(db_path);
        }
        scope
    }
}

/// The runtime's default data directory: `$KERNEX_DATA_DIR`, or `~/.kernex`.
fn default_data_dir() -> PathBuf {
    let dir = std::env::var(DATA_DIR_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| kernex_core::config::RuntimeConfig::default().data_dir);
    PathBuf::from(kernex_core::shellexpand(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_uses_the_configured_directories() {
        let dirs = SandboxDirs::new()
            .with_data_dir("/srv/agent/state")
            .with_config_path("/etc/agent/config.toml")
            .with_db_path("/var/lib/agent/memory.db");
        let scope = dirs.scope(Path::new("/srv/agent/work"));
        assert_eq!(scope.workspace, PathBuf::from("/srv/agent/work"));
        assert_eq!(scope.data_dir, PathBuf::from("/srv/agent/state"));
        assert_eq!(
            scope.config_path,
            Some(PathBuf::from("/etc/agent/config.toml"))
        );
        assert_eq!(
            scope.db_path,
            Some(PathBuf::from("/var/lib/agent/memory.db"))
        );
    }

    #[test]
    fn test_scope_never_derives_the_data_dir_from_the_workspace() {
        // With no data dir configured the scope falls back to the runtime
        // default, not to the parent of the workspace.
        let scope = SandboxDirs::new().scope(Path::new("/srv/agent/work"));
        assert_ne!(scope.data_dir, PathBuf::from("/srv/agent"));
        assert!(scope.data_dir.is_absolute());
    }
}
