//! kernex-memory: SQLite-backed implementation of the `Store` trait.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides conversation storage, reward-based learning (outcomes + lessons),
//! scheduled tasks, FTS5 semantic recall, and audit logging.

pub mod audit;
pub mod consolidator;
pub mod store;

pub use audit::AuditLogger;
pub use consolidator::{ConsolidationResult, Consolidator, ConsolidatorConfig};
pub use store::detect_language;
pub use store::DueTask;
pub use store::PhaseCheckpoint;
pub use store::Store;
pub use store::UsageSummary;
