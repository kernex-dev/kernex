//! Per-crate error type for `kernex-memory`.
//!
//! Replaces the old `kernex_core::KernexError::Store(String)` shape so callers
//! can pattern-match on the actual cause (e.g. distinguish a connection
//! failure from a missing-row condition). Foreign errors (`sqlx::Error`,
//! `std::io::Error`) are preserved as `#[source]` so the chain is intact.

/// Errors produced by the memory store.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// A SQLite operation failed. `context` describes what we were trying to
    /// do; `source` is the underlying `sqlx::Error` for chain inspection.
    #[error("{context}: {source}")]
    Sqlite {
        /// Human-readable description of the failing operation
        /// (e.g. "advance task", "fts search", "record token usage").
        context: String,
        /// The underlying sqlx error.
        #[source]
        source: sqlx::Error,
    },

    /// A filesystem operation failed. `context` describes what we were trying
    /// to do; `source` is the underlying `std::io::Error`.
    #[error("{context}: {source}")]
    Io {
        /// Human-readable description of the failing operation
        /// (e.g. "create data dir", "open audit log").
        context: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("{context}: {source}")]
    Serde {
        /// Human-readable description of the failing operation.
        context: String,
        /// The underlying serde_json error.
        #[source]
        source: serde_json::Error,
    },

    /// Domain-logic error with no foreign source — invalid arguments,
    /// missing rows, malformed input, etc.
    #[error("{0}")]
    Logic(String),
}

impl MemoryError {
    /// Wrap a `sqlx::Error` with operation context.
    pub fn sqlite(context: impl Into<String>, source: sqlx::Error) -> Self {
        Self::Sqlite {
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

    /// Wrap a `serde_json::Error` with operation context.
    pub fn serde(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Serde {
            context: context.into(),
            source,
        }
    }

    /// Construct a domain-logic error from a message.
    pub fn logic(msg: impl Into<String>) -> Self {
        Self::Logic(msg.into())
    }
}

/// Bridge to the workspace-level aggregate error.
///
/// Boxes the typed `MemoryError` inside `KernexError::Store` so callers
/// downstream can recover the structured cause via
/// `boxed.downcast_ref::<MemoryError>()` and pattern-match on the
/// concrete variant (e.g. `MemoryError::Sqlite { source, .. }` to inspect
/// the underlying `sqlx::Error`).
impl From<MemoryError> for kernex_core::error::KernexError {
    fn from(err: MemoryError) -> Self {
        kernex_core::error::KernexError::store(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_display_includes_context_and_source() {
        let inner = sqlx::Error::PoolTimedOut;
        let err = MemoryError::sqlite("acquire conn", inner);
        let msg = format!("{err}");
        assert!(msg.contains("acquire conn"), "msg was {msg:?}");
        assert!(msg.contains("pool"), "msg was {msg:?}");
    }

    #[test]
    fn logic_display_passthrough() {
        let err = MemoryError::logic("session not found");
        assert_eq!(format!("{err}"), "session not found");
    }
}
