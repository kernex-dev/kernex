//! kernex-memory: SQLite-backed implementation of the `Store` trait.
//!
//! Provides conversation storage, reward-based learning (outcomes + lessons),
//! scheduled tasks, FTS5 semantic recall, and audit logging.

pub mod audit;
pub mod store;

pub use audit::AuditLogger;
pub use store::detect_language;
pub use store::DueTask;
pub use store::Store;
