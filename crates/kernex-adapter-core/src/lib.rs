//! Core trait and supporting types for kernex agent adapters.
//!
//! Workspace-internal. Concrete adapter implementations land in follow-up
//! changes; this crate defines the shape they target.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::sync::Arc;

use thiserror::Error;

/// Stable identifier for a supported agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum AdapterId {
    ClaudeCode,
    CodexCli,
    OpenCode,
    Cursor,
    Cline,
}

/// Capability surface an adapter exposes. Sync default methods so adapter
/// authors can override without dragging async machinery into capability
/// reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Capability {
    Skills,
    Memory,
    Mcp,
    OutputStyle,
}

/// Lightweight detection result surfaced by [`Adapter::detect`].
///
/// `config_root` is the adapter's home-rooted configuration directory
/// (e.g., `~/.claude` for Claude Code, `~/.codex` for Codex CLI).
/// `project_root` is the project-local allowlisted root for adapters that
/// also write files in the current working directory (e.g., Codex's
/// `<cwd>/AGENTS.md`, Cursor's `.cursorrules`). The Stage 5 APPLY sandbox
/// check accepts writes inside EITHER `config_root` OR `project_root`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Detection {
    pub installed: bool,
    pub config_root: Option<std::path::PathBuf>,
    #[serde(default)]
    pub project_root: Option<std::path::PathBuf>,
    pub version: Option<String>,
}

impl Detection {
    /// Construct a `Detection` for an adapter with no project-local writes.
    ///
    /// `project_root` is set to `None`. Adapters that write project-local
    /// files (e.g., Codex `<cwd>/AGENTS.md`, Cursor `.cursorrules`) should
    /// use [`Detection::with_project_root`] instead.
    ///
    /// The type is `#[non_exhaustive]`, so external crates cannot use a
    /// struct literal. This constructor is the additive public surface that
    /// lets downstream consumers build the value with a single call.
    pub fn new(
        installed: bool,
        config_root: Option<std::path::PathBuf>,
        version: Option<String>,
    ) -> Self {
        Self {
            installed,
            config_root,
            project_root: None,
            version,
        }
    }

    /// Construct a `Detection` for an adapter that writes both home-rooted
    /// and project-local files.
    ///
    /// Example: Codex writes `~/.codex/config.toml` (home) plus
    /// `<cwd>/AGENTS.md` (project); Cursor writes `~/.cursor/mcp.json`
    /// (home) plus `<cwd>/.cursorrules` (project). The Stage 5 APPLY
    /// sandbox check accepts writes inside EITHER `config_root` OR
    /// `project_root`.
    pub fn with_project_root(
        installed: bool,
        config_root: Option<std::path::PathBuf>,
        project_root: Option<std::path::PathBuf>,
        version: Option<String>,
    ) -> Self {
        Self {
            installed,
            config_root,
            project_root,
            version,
        }
    }
}

/// Adapter error type. `#[non_exhaustive]` so future variants are non-breaking.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdapterError {
    #[error("adapter id {0:?} is not supported in this build")]
    Unsupported(AdapterId),

    #[error("config root unreadable: {0}")]
    ConfigRootUnreadable(std::path::PathBuf),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Adapter trait. Object-safe; pin async to I/O methods only.
#[async_trait::async_trait]
pub trait Adapter: Send + Sync {
    fn id(&self) -> AdapterId;

    fn supports(&self, _cap: Capability) -> bool {
        false
    }

    async fn detect(&self) -> Result<Detection, AdapterError>;

    async fn install_command(&self) -> Result<String, AdapterError>;
}

/// Default adapter set known to this build. Empty in this scaffold; follow-up
/// changes add entries as concrete adapter implementations land.
pub const DEFAULT_ADAPTER_IDS: &[AdapterId] = &[];

/// Switch-arm factory. Closed match; adding a new `AdapterId` variant breaks
/// the build until this function is updated.
pub fn new_adapter(id: AdapterId) -> Result<Arc<dyn Adapter>, AdapterError> {
    match id {
        AdapterId::ClaudeCode
        | AdapterId::CodexCli
        | AdapterId::OpenCode
        | AdapterId::Cursor
        | AdapterId::Cline => Err(AdapterError::Unsupported(id)),
    }
}

/// Registry of adapter handles, keyed by [`AdapterId`].
#[derive(Default)]
pub struct AdapterRegistry {
    inner: std::collections::HashMap<AdapterId, Arc<dyn Adapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter handle keyed by its [`AdapterId`]. Returns the
    /// previous handle for that id if one was already registered, mirroring
    /// [`std::collections::HashMap::insert`]. Callers can detect duplicate
    /// registrations by checking for `Some(_)`.
    pub fn register(&mut self, adapter: Arc<dyn Adapter>) -> Option<Arc<dyn Adapter>> {
        self.inner.insert(adapter.id(), adapter)
    }

    pub fn get(&self, id: AdapterId) -> Option<Arc<dyn Adapter>> {
        self.inner.get(&id).cloned()
    }
}

/// Build a registry pre-populated with [`DEFAULT_ADAPTER_IDS`]. Empty in this
/// scaffold; follow-up changes populate it as adapter implementations land.
pub fn default_registry() -> Result<AdapterRegistry, AdapterError> {
    let mut registry = AdapterRegistry::new();
    for id in DEFAULT_ADAPTER_IDS {
        let _ = registry.register(new_adapter(*id)?);
    }
    Ok(registry)
}
