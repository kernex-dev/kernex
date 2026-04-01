//! Background memory consolidation (AutoDream pattern).
//!
//! Three-gate design: time gate (hours since last run), session gate
//! (new conversations since last run), and in-process lock gate
//! (prevent concurrent runs). State persists in the `facts` table
//! under `sender_id = "__system__"`.
//!
//! # Example
//!
//! ```rust,ignore
//! use kernex_memory::{Store, consolidator::{Consolidator, ConsolidatorConfig}};
//!
//! let store = Store::new(&config).await?;
//! let consolidator = Consolidator::new(store.clone(), ConsolidatorConfig::default());
//! consolidator.spawn(600); // check every 10 minutes
//! ```

use crate::Store;
use kernex_core::error::KernexError;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Sender ID used for system-level metadata stored in the `facts` table.
const SYSTEM_SENDER: &str = "__system__";
/// Fact key for last consolidation Unix timestamp (seconds, stored as string).
const LAST_CONSOLIDATED_KEY: &str = "last_consolidated_at";

/// Configuration for the memory consolidator.
#[derive(Debug, Clone)]
pub struct ConsolidatorConfig {
    /// Minimum hours between consolidation runs (default: 24).
    pub min_hours: f64,
    /// Minimum new conversations since last run required to proceed (default: 5).
    pub min_sessions: usize,
    /// Delete messages from closed conversations older than this many days (default: 30).
    pub max_message_age_days: u64,
    /// Maximum outcomes to retain per sender (excess oldest rows are deleted, default: 100).
    pub max_outcomes_per_sender: usize,
}

impl Default for ConsolidatorConfig {
    fn default() -> Self {
        Self {
            min_hours: 24.0,
            min_sessions: 5,
            max_message_age_days: 30,
            max_outcomes_per_sender: 100,
        }
    }
}

/// Statistics from a single consolidation run.
#[derive(Debug, Default)]
pub struct ConsolidationResult {
    /// Number of message rows deleted.
    pub messages_pruned: u64,
    /// Number of outcome rows deleted.
    pub outcomes_pruned: u64,
}

/// Background memory consolidator with three-gate activation.
///
/// Holds a `Store` clone and an in-process `Mutex` so concurrent calls to
/// [`maybe_run`](Self::maybe_run) skip gracefully instead of racing.
pub struct Consolidator {
    store: Store,
    config: ConsolidatorConfig,
    lock: Arc<Mutex<()>>,
}

impl Consolidator {
    /// Create a new consolidator with the given store and config.
    pub fn new(store: Store, config: ConsolidatorConfig) -> Self {
        Self {
            store,
            config,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Run consolidation only if all three gates pass.
    ///
    /// Returns `Ok(None)` if any gate rejects the run.
    pub async fn maybe_run(&self) -> Result<Option<ConsolidationResult>, KernexError> {
        // Lock gate: skip if another run is already in progress.
        let _guard = match self.lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::debug!("consolidator: lock gate rejected (run in progress)");
                return Ok(None);
            }
        };

        let last_ts = self.last_consolidated_at().await?;

        // Time gate.
        if !self.time_gate_passes(last_ts) {
            return Ok(None);
        }

        // Session gate.
        let new_sessions = self.count_new_sessions(last_ts).await?;
        if new_sessions < self.config.min_sessions {
            tracing::debug!(
                "consolidator: session gate rejected ({new_sessions} < {})",
                self.config.min_sessions
            );
            return Ok(None);
        }

        info!("consolidator: all gates passed (new_sessions={new_sessions}), pruning");

        let result = self.prune().await?;

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.store
            .store_fact(SYSTEM_SENDER, LAST_CONSOLIDATED_KEY, &now_secs.to_string())
            .await?;

        Ok(Some(result))
    }

    /// Run pruning unconditionally, bypassing all gates.
    ///
    /// Useful for forcing a consolidation on startup or for testing.
    pub async fn prune(&self) -> Result<ConsolidationResult, KernexError> {
        let messages_pruned = self.prune_old_messages().await?;
        let outcomes_pruned = self.prune_excess_outcomes().await?;

        info!("consolidator: pruned {messages_pruned} messages, {outcomes_pruned} outcomes");

        Ok(ConsolidationResult {
            messages_pruned,
            outcomes_pruned,
        })
    }

    /// Spawn a background task that calls [`maybe_run`](Self::maybe_run) every `interval_secs`.
    ///
    /// The returned `JoinHandle` can be dropped — the task continues until
    /// the process exits. Call `handle.abort()` to cancel.
    pub fn spawn(self, interval_secs: u64) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                ticker.tick().await;
                match self.maybe_run().await {
                    Ok(Some(r)) => info!(
                        "consolidator: pruned {} messages, {} outcomes",
                        r.messages_pruned, r.outcomes_pruned
                    ),
                    Ok(None) => {}
                    Err(e) => warn!("consolidator background run error: {e}"),
                }
            }
        })
    }

    // --- Private helpers ---

    async fn last_consolidated_at(&self) -> Result<Option<u64>, KernexError> {
        Ok(self
            .store
            .get_fact(SYSTEM_SENDER, LAST_CONSOLIDATED_KEY)
            .await?
            .and_then(|v| v.parse::<u64>().ok()))
    }

    fn time_gate_passes(&self, last_ts: Option<u64>) -> bool {
        let Some(ts) = last_ts else { return true };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let elapsed_hours = now.saturating_sub(ts) as f64 / 3600.0;
        if elapsed_hours < self.config.min_hours {
            tracing::debug!(
                "consolidator: time gate rejected ({elapsed_hours:.1}h < {}h)",
                self.config.min_hours
            );
            return false;
        }
        true
    }

    async fn count_new_sessions(&self, since_ts: Option<u64>) -> Result<usize, KernexError> {
        let (count,): (i64,) = match since_ts {
            None => {
                sqlx::query_as("SELECT COUNT(*) FROM conversations")
                    .fetch_one(self.store.pool())
                    .await
            }
            Some(ts) => {
                sqlx::query_as(
                    "SELECT COUNT(*) FROM conversations \
                 WHERE started_at > datetime(?, 'unixepoch')",
                )
                .bind(ts as i64)
                .fetch_one(self.store.pool())
                .await
            }
        }
        .map_err(|e| KernexError::Store(format!("count sessions: {e}")))?;

        Ok(count as usize)
    }

    async fn prune_old_messages(&self) -> Result<u64, KernexError> {
        let age_days = -(self.config.max_message_age_days as i64);
        let r = sqlx::query(
            "DELETE FROM messages WHERE conversation_id IN ( \
                 SELECT id FROM conversations \
                 WHERE status = 'closed' \
                 AND datetime(updated_at) < datetime('now', ? || ' days') \
             )",
        )
        .bind(age_days)
        .execute(self.store.pool())
        .await
        .map_err(|e| KernexError::Store(format!("prune messages: {e}")))?;
        Ok(r.rows_affected())
    }

    async fn prune_excess_outcomes(&self) -> Result<u64, KernexError> {
        let max = self.config.max_outcomes_per_sender as i64;
        let senders: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT sender_id FROM outcomes")
            .fetch_all(self.store.pool())
            .await
            .map_err(|e| KernexError::Store(format!("get outcome senders: {e}")))?;

        let mut deleted = 0u64;
        for (sender_id,) in senders {
            let r = sqlx::query(
                "DELETE FROM outcomes WHERE id IN ( \
                     SELECT id FROM outcomes WHERE sender_id = ? \
                     ORDER BY timestamp DESC LIMIT -1 OFFSET ? \
                 )",
            )
            .bind(&sender_id)
            .bind(max)
            .execute(self.store.pool())
            .await
            .map_err(|e| KernexError::Store(format!("prune outcomes for {sender_id}: {e}")))?;
            deleted += r.rows_affected();
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernex_core::config::MemoryConfig;

    async fn make_store() -> Store {
        let config = MemoryConfig {
            db_path: ":memory:".to_string(),
            ..Default::default()
        };
        Store::new(&config).await.unwrap()
    }

    #[tokio::test]
    async fn test_time_gate_passes_no_prior_run() {
        let store = make_store().await;
        let c = Consolidator::new(store, ConsolidatorConfig::default());
        // No prior timestamp -> gate passes.
        assert!(c.time_gate_passes(None));
    }

    #[tokio::test]
    async fn test_time_gate_rejects_too_recent() {
        let store = make_store().await;
        let c = Consolidator::new(store, ConsolidatorConfig::default());
        // Timestamp just now -> not enough hours have elapsed.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(!c.time_gate_passes(Some(now)));
    }

    #[tokio::test]
    async fn test_time_gate_passes_old_run() {
        let store = make_store().await;
        let c = Consolidator::new(
            store,
            ConsolidatorConfig {
                min_hours: 1.0,
                ..Default::default()
            },
        );
        // Timestamp 2 hours ago -> gate passes.
        let two_hours_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7200;
        assert!(c.time_gate_passes(Some(two_hours_ago)));
    }

    #[tokio::test]
    async fn test_prune_no_data_returns_zero() {
        let store = make_store().await;
        let c = Consolidator::new(store, ConsolidatorConfig::default());
        let result = c.prune().await.unwrap();
        assert_eq!(result.messages_pruned, 0);
        assert_eq!(result.outcomes_pruned, 0);
    }

    #[tokio::test]
    async fn test_maybe_run_session_gate_rejects_empty_store() {
        let store = make_store().await;
        let c = Consolidator::new(
            store,
            ConsolidatorConfig {
                min_sessions: 5,
                min_hours: 0.0, // pass time gate immediately
                ..Default::default()
            },
        );
        // No conversations -> session count (0) < min_sessions (5) -> rejected.
        let result = c.maybe_run().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_state_persisted_after_run() {
        let store = make_store().await;
        // Seed enough conversations to pass the session gate.
        for i in 0..6 {
            sqlx::query(
                "INSERT INTO conversations (id, channel, sender_id, status, last_activity) \
                 VALUES (?, 'test', 'user1', 'active', datetime('now'))",
            )
            .bind(format!("conv-{i}"))
            .execute(store.pool())
            .await
            .unwrap();
        }

        let c = Consolidator::new(
            store.clone(),
            ConsolidatorConfig {
                min_sessions: 5,
                min_hours: 0.0,
                ..Default::default()
            },
        );
        let result = c.maybe_run().await.unwrap();
        assert!(result.is_some(), "expected consolidation to run");

        // State should now be persisted.
        let ts = store
            .get_fact(SYSTEM_SENDER, LAST_CONSOLIDATED_KEY)
            .await
            .unwrap();
        assert!(ts.is_some(), "last_consolidated_at should be stored");
    }

    #[tokio::test]
    async fn test_lock_gate_prevents_concurrent_runs() {
        let store = make_store().await;
        let c = Consolidator::new(store, ConsolidatorConfig::default());

        // Acquire the lock manually to simulate a run in progress.
        let _guard = c.lock.try_lock().unwrap();
        let result = c.maybe_run().await.unwrap();
        assert!(result.is_none(), "lock gate should reject");
    }

    #[tokio::test]
    async fn test_prune_excess_outcomes() {
        let store = make_store().await;
        // Insert 5 outcomes for user1.
        for i in 0..5 {
            sqlx::query(
                "INSERT INTO outcomes (id, sender_id, domain, score, lesson, source, project) \
                 VALUES (?, 'user1', 'test', 1, 'lesson', 'src', '')",
            )
            .bind(format!("out-{i}"))
            .execute(store.pool())
            .await
            .unwrap();
        }

        let c = Consolidator::new(
            store.clone(),
            ConsolidatorConfig {
                max_outcomes_per_sender: 3,
                ..Default::default()
            },
        );
        let deleted = c.prune_excess_outcomes().await.unwrap();
        assert_eq!(deleted, 2, "should have pruned 2 excess outcomes");

        let (remaining,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM outcomes WHERE sender_id = 'user1'")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(remaining, 3);
    }
}
