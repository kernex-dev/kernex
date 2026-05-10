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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Detection {
    pub installed: bool,
    pub config_root: Option<std::path::PathBuf>,
    pub version: Option<String>,
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

    pub fn register(&mut self, adapter: Arc<dyn Adapter>) {
        self.inner.insert(adapter.id(), adapter);
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
        registry.register(new_adapter(*id)?);
    }
    Ok(registry)
}
