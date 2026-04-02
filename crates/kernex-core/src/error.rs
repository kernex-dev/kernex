//! Unified error type for all Kernex crates.

/// Top-level error type for Kernex.
#[derive(Debug, thiserror::Error)]
pub enum KernexError {
    /// Error from an AI provider.
    #[error("provider error: {0}")]
    Provider(String),

    /// Memory/storage error.
    #[error("store error: {0}")]
    Store(String),

    /// Sandbox execution error.
    #[error("sandbox error: {0}")]
    Sandbox(String),

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// Pipeline execution error.
    #[error("pipeline error: {0}")]
    Pipeline(String),

    /// Skill loading/matching error.
    #[error("skill error: {0}")]
    Skill(String),

    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Serialization error.
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),

    /// Guardrail blocked the request or response.
    #[error("guardrail blocked: {0}")]
    Guardrail(String),
}

pub type Result<T> = std::result::Result<T, KernexError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = KernexError::from(io_err);
        let display = format!("{err}");
        assert!(display.contains("file missing"));
    }

    #[test]
    fn test_provider_error_display() {
        let err = KernexError::Provider("timeout".into());
        assert_eq!(format!("{err}"), "provider error: timeout");
    }

    #[test]
    fn test_config_error_display() {
        let err = KernexError::Config("missing field".into());
        assert_eq!(format!("{err}"), "config error: missing field");
    }

    #[test]
    fn test_store_error_display() {
        let err = KernexError::Store("connection failed".into());
        assert_eq!(format!("{err}"), "store error: connection failed");
    }
}
