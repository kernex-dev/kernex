//! kernex-core: Foundation types, traits, and error handling for the Kernex runtime.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod context;
pub mod error;
pub mod hooks;
pub mod message;
pub mod run;
pub mod sanitize;
pub mod stream;
pub mod traits;

pub use config::shellexpand;
pub use error::KernexError;
pub use run::ModelTier;
