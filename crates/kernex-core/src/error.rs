//! Unified error type for all Kernex crates.
//!
//! # Architecture (0.5.0)
//!
//! `KernexError` is the workspace-level aggregate. Variants whose cause
//! originates in a sibling crate (`Provider`, `Store`, `Pipeline`, `Skill`,
//! `Sandbox`) carry a boxed `std::error::Error` trait object so this crate
//! never has to depend on `kernex-providers`, `kernex-memory`, etc. — that
//! would create a dependency cycle since those crates depend on
//! `kernex-core`. Each per-crate crate provides its own typed `thiserror`
//! enum (`ProviderError`, `MemoryError`, …) and an
//! `impl From<TheirError> for KernexError` that boxes the typed error.
//!
//! Callers wanting to recover the typed cause use `downcast_ref::<T>()`:
//!
//! ```rust,no_run
//! # use kernex_core::error::KernexError;
//! # let err: KernexError = todo!();
//! match &err {
//!     KernexError::Provider(boxed) => {
//!         // if let Some(p) = boxed.downcast_ref::<kernex_providers::ProviderError>() {
//!         //     match p { /* typed variants */ }
//!         // }
//!     }
//!     _ => {}
//! }
//! ```
//!
//! This is the same boxed-trait-object pattern used by `tower::Service`
//! middleware and recommended by the Rust API Guidelines (C-GOOD-ERR).

/// A boxed, type-erased error trait object satisfying the bounds Rust's
/// async ecosystem expects (`Send + Sync + 'static`). Used as the inner
/// payload for the cross-crate `KernexError` variants so this crate stays
/// free of `kernex-providers` / `kernex-memory` / etc. dependencies.
pub type BoxedError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Top-level error type for Kernex.
///
/// Cross-crate variants (`Provider`, `Store`, `Sandbox`, `Pipeline`, `Skill`)
/// carry the originating typed error inside a [`BoxedError`]; recover the
/// concrete type with [`std::error::Error::downcast_ref`] on the boxed value.
/// Crate-local variants (`Config`, `Guardrail`) carry a plain `String` since
/// they have no foreign source to preserve.
#[derive(Debug, thiserror::Error)]
pub enum KernexError {
    /// Error from an AI provider. Inner error is typically a
    /// `kernex_providers::ProviderError`; recover the typed variant via
    /// `boxed.downcast_ref::<ProviderError>()`.
    #[error(transparent)]
    Provider(BoxedError),

    /// Memory/storage error. Inner error is typically a
    /// `kernex_memory::MemoryError`.
    #[error(transparent)]
    Store(BoxedError),

    /// Sandbox execution error. Inner error originates in `kernex-sandbox`.
    #[error(transparent)]
    Sandbox(BoxedError),

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// Pipeline execution error. Inner error is typically a
    /// `kernex_pipelines::PipelineError`.
    #[error(transparent)]
    Pipeline(BoxedError),

    /// Skill loading/matching error. Inner error is typically a
    /// `kernex_skills::SkillError`.
    #[error(transparent)]
    Skill(BoxedError),

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

impl KernexError {
    /// Convenience constructor: box `e` as a [`KernexError::Provider`].
    pub fn provider<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        KernexError::Provider(Box::new(e))
    }

    /// Convenience constructor: box `e` as a [`KernexError::Store`].
    pub fn store<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        KernexError::Store(Box::new(e))
    }

    /// Convenience constructor: box `e` as a [`KernexError::Sandbox`].
    pub fn sandbox<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        KernexError::Sandbox(Box::new(e))
    }

    /// Convenience constructor: box `e` as a [`KernexError::Pipeline`].
    pub fn pipeline<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        KernexError::Pipeline(Box::new(e))
    }

    /// Convenience constructor: box `e` as a [`KernexError::Skill`].
    pub fn skill<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        KernexError::Skill(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, KernexError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_display_passes_through() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = KernexError::from(io_err);
        let display = format!("{err}");
        assert!(display.contains("file missing"));
    }

    #[test]
    fn config_error_display() {
        let err = KernexError::Config("missing field".into());
        assert_eq!(format!("{err}"), "config error: missing field");
    }

    #[test]
    fn guardrail_error_display() {
        let err = KernexError::Guardrail("blocked".into());
        assert_eq!(format!("{err}"), "guardrail blocked: blocked");
    }

    /// A toy typed error to exercise the boxed-variant downcast pattern.
    #[derive(Debug, thiserror::Error)]
    enum ToyError {
        #[error("network: {0}")]
        Network(String),
        #[error("parse: {0}")]
        Parse(String),
    }

    #[test]
    fn provider_variant_round_trips_typed_error_via_downcast() {
        let original = ToyError::Network("connection refused".into());
        let err = KernexError::provider(original);

        // Display goes through transparent → underlying ToyError display.
        assert!(format!("{err}").contains("connection refused"));

        // Downcast recovers the typed variant for pattern matching.
        match &err {
            KernexError::Provider(boxed) => {
                let typed = boxed
                    .downcast_ref::<ToyError>()
                    .expect("boxed value must be ToyError");
                assert!(matches!(typed, ToyError::Network(_)));
            }
            other => panic!("expected Provider, got {other:?}"),
        }
    }

    #[test]
    fn store_variant_round_trips_typed_error_via_downcast() {
        let original = ToyError::Parse("bad json".into());
        let err = KernexError::store(original);
        match &err {
            KernexError::Store(boxed) => {
                assert!(matches!(
                    boxed.downcast_ref::<ToyError>(),
                    Some(ToyError::Parse(_))
                ));
            }
            other => panic!("expected Store, got {other:?}"),
        }
    }
}
