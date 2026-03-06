//! kernex-core: Foundation types, traits, and error handling for the Kernex runtime.

pub mod config;
pub mod context;
pub mod error;
pub mod message;
pub mod sanitize;
pub mod traits;

pub use config::shellexpand;
pub use error::KernexError;
