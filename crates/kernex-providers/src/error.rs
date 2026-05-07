//! Per-crate error type for `kernex-providers`.
//!
//! Replaces the old `kernex_core::KernexError::Provider(String)` shape so
//! callers can pattern-match on the actual cause (e.g. distinguish a network
//! failure from a JSON parse error). Foreign errors (`reqwest::Error`,
//! `serde_json::Error`, `std::io::Error`) are preserved as `#[source]` so
//! the chain stays intact.

/// Errors produced by AI provider implementations and the shared tool loop.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// HTTP transport failure — request build, network error, timeout.
    /// `context` describes the operation; `source` is the `reqwest::Error`.
    #[error("{context}: {source}")]
    Http {
        /// Human-readable description of the failing operation
        /// (e.g. "anthropic: stream request", "build HTTP client").
        context: String,
        /// The underlying reqwest error.
        #[source]
        source: reqwest::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("{context}: {source}")]
    Serde {
        /// Human-readable description (e.g. "anthropic: parse response").
        context: String,
        /// The underlying serde_json error.
        #[source]
        source: serde_json::Error,
    },

    /// A filesystem operation failed.
    #[error("{context}: {source}")]
    Io {
        /// Human-readable description (e.g. "create .claude dir",
        /// "write MCP settings").
        context: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Missing or invalid configuration — env var unset, bad model id, etc.
    #[error("{0}")]
    Config(String),

    /// Domain-logic error with no foreign source — provider rejected the
    /// request, retries exhausted, response shape mismatch, etc.
    #[error("{0}")]
    Logic(String),
}

impl ProviderError {
    /// Wrap a `reqwest::Error` with operation context.
    pub fn http(context: impl Into<String>, source: reqwest::Error) -> Self {
        Self::Http {
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

    /// Wrap a `std::io::Error` with operation context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Construct a config error from a message.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Construct a domain-logic error from a message.
    pub fn logic(msg: impl Into<String>) -> Self {
        Self::Logic(msg.into())
    }
}

/// Bridge to the workspace-level aggregate error.
///
/// Boxes the typed `ProviderError` inside `KernexError::Provider` so callers
/// downstream can recover the structured cause via
/// `boxed.downcast_ref::<ProviderError>()`. `Config` is hoisted to the
/// dedicated `KernexError::Config` variant since it's a configuration
/// failure, not a provider failure.
impl From<ProviderError> for kernex_core::error::KernexError {
    fn from(err: ProviderError) -> Self {
        match err {
            ProviderError::Config(msg) => kernex_core::error::KernexError::Config(msg),
            other => kernex_core::error::KernexError::provider(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_display_passthrough() {
        let err = ProviderError::config("AWS_ACCESS_KEY_ID not set");
        assert_eq!(format!("{err}"), "AWS_ACCESS_KEY_ID not set");
    }

    #[test]
    fn logic_display_passthrough() {
        let err = ProviderError::logic("retries exhausted");
        assert_eq!(format!("{err}"), "retries exhausted");
    }
}
