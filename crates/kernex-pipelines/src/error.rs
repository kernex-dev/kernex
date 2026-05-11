//! Per-crate error type for `kernex-pipelines`.
//!
//! Replaces the old `kernex_core::KernexError::Pipeline(String)` shape so
//! callers can pattern-match on the actual cause. Foreign errors
//! (`basic_toml::Error`, `std::io::Error`) are preserved as `#[source]` so
//! the chain stays intact.

/// Errors produced by topology loading and pipeline execution.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// TOML parse failure on `TOPOLOGY.toml` or related files.
    #[error("{context}: {source}")]
    Toml {
        /// Human-readable description of the failing operation.
        context: String,
        /// The underlying toml deserialization error.
        #[source]
        source: basic_toml::Error,
    },

    /// A filesystem operation failed (read TOPOLOGY.toml, list agents/, etc).
    #[error("{context}: {source}")]
    Io {
        /// Human-readable description of the failing operation.
        context: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Domain-logic error — invalid topology, missing agent, validation
    /// failure, etc.
    #[error("{0}")]
    Logic(String),
}

impl PipelineError {
    /// Wrap a `basic_toml::Error` with operation context.
    pub fn toml(context: impl Into<String>, source: basic_toml::Error) -> Self {
        Self::Toml {
            context: context.into(),
            source,
        }
    }

    /// Wrap a `std::io::Error` with operation context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Construct a domain-logic error from a message.
    pub fn logic(msg: impl Into<String>) -> Self {
        Self::Logic(msg.into())
    }
}

/// Bridge to the workspace-level aggregate error. Boxes the typed
/// `PipelineError` inside `KernexError::Pipeline` so callers downstream
/// can recover the structured cause via
/// `boxed.downcast_ref::<PipelineError>()`.
impl From<PipelineError> for kernex_core::error::KernexError {
    fn from(err: PipelineError) -> Self {
        kernex_core::error::KernexError::pipeline(err)
    }
}
