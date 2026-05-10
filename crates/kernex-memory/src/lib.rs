#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! kernex-memory: SQLite-backed memory store with a public trait surface.
//!
//! Provides conversation storage, reward-based learning (outcomes + lessons),
//! scheduled tasks, FTS5 semantic recall, audit logging, and soft-delete on
//! the `facts` table. The public [`MemoryStore`] trait mirrors the inherent
//! method surface that downstream consumers call today; concrete schema
//! changes can ship without rippling into call sites.

pub mod audit;
pub mod consolidator;
pub mod error;
pub mod memory_store;
pub mod store;

pub use audit::AuditLogger;
pub use consolidator::{ConsolidationResult, Consolidator, ConsolidatorConfig};
pub use error::MemoryError;
pub use memory_store::{into_handle, MemoryStore};
pub use store::detect_language;
pub use store::DueTask;
pub use store::PhaseCheckpoint;
pub use store::Store;
pub use store::{UsageBreakdown, UsageSummary};
