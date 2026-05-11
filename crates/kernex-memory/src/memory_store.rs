#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Public trait surface for the SQLite-backed memory store.
//!
//! Mirrors the inherent method surface that downstream consumers
//! (`kernex-runtime` composition, the sister-repo binary's REPL, future
//! CLI/HTTP/MCP) call today, plus three new soft-delete methods on the
//! `facts` table. Hard-delete inherent methods (`delete_fact`,
//! `delete_facts`) stay on `Store` for emergency cleanup tooling and are
//! deliberately NOT on the trait so the default consumer path uses
//! recoverable soft-delete.
//!
//! `Runtime::store_handle()` returns `Arc<dyn MemoryStore>` so a binary
//! consumer can share the runtime's composed `Store` instance instead of
//! opening a second SQLite connection against the same database file.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::store::{DueTask, Store, UsageSummary};
use crate::types::{HistoryRow, MessageRow};

/// Public trait surface over [`Store`].
///
/// Returned from `kernex-runtime::Runtime::store_handle()` as
/// `Arc<dyn MemoryStore>`. Consumers should prefer this trait over the
/// concrete `Store` type so future schema changes do not ripple into call
/// sites.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // --- conversations / messages ---

    /// Mark the active conversation for `(channel, sender_id, project)` as
    /// closed. Returns `true` if a row transitioned from active to closed.
    async fn close_current_conversation(
        &self,
        channel: &str,
        sender_id: &str,
        project: &str,
    ) -> Result<bool, MemoryError>;

    /// Aggregate counters: `(conversation_count, message_count, fact_count)`.
    async fn get_memory_stats(&self, sender_id: &str) -> Result<(i64, i64, i64), MemoryError>;

    /// On-disk byte size of the SQLite database file.
    async fn db_size(&self) -> Result<u64, MemoryError>;

    /// Aggregate token usage across all sessions.
    async fn get_total_usage(&self) -> Result<UsageSummary, MemoryError>;

    /// Recent closed-conversation summaries for a given channel + sender,
    /// newest first, capped at `limit`.
    async fn get_history(
        &self,
        channel: &str,
        sender_id: &str,
        limit: i64,
    ) -> Result<Vec<HistoryRow>, MemoryError>;

    /// FTS5 full-text search over user messages, excluding the live
    /// conversation. When `since` is `Some`, only rows with
    /// `timestamp >= since` are returned and `limit` applies after the
    /// recency filter.
    async fn search_messages(
        &self,
        query: &str,
        exclude_conversation_id: &str,
        sender_id: &str,
        limit: i64,
        since: Option<SystemTime>,
    ) -> Result<Vec<MessageRow>, MemoryError>;

    /// Fetch a single message row by its UUID. Returns `None` when the
    /// id is missing.
    async fn get_message_by_id(&self, id: &str) -> Result<Option<MessageRow>, MemoryError>;

    // --- facts (write paths plus soft-only delete on the trait) ---

    /// Upsert a fact for `(sender_id, key)`. If the row was previously
    /// soft-deleted, this clears `deleted_at` so the value is visible
    /// again to default-filtered reads.
    async fn store_fact(&self, sender_id: &str, key: &str, value: &str) -> Result<(), MemoryError>;

    /// Read a single active fact by `(sender_id, key)`. Returns `None` if
    /// the row is soft-deleted, missing, or never existed.
    async fn get_fact(&self, sender_id: &str, key: &str) -> Result<Option<String>, MemoryError>;

    /// Active (not soft-deleted) facts for `sender_id`.
    async fn get_facts(&self, sender_id: &str) -> Result<Vec<(String, String)>, MemoryError>;

    /// Soft-delete a single fact by setting its `deleted_at` timestamp.
    /// Returns `true` if a row transitioned from active to deleted; `false`
    /// if the row was already deleted, missing, or never existed.
    async fn soft_delete_fact(&self, sender_id: &str, key: &str) -> Result<bool, MemoryError>;

    /// Soft-delete multiple facts. With `Some(key)`, soft-deletes that
    /// specific key. With `None`, soft-deletes every active fact for the
    /// sender. Returns the count of rows that transitioned from active to
    /// deleted.
    async fn soft_delete_facts(
        &self,
        sender_id: &str,
        key: Option<&str>,
    ) -> Result<u64, MemoryError>;

    /// Read soft-deleted facts (debug / recovery helper). Returns
    /// `(key, value, deleted_at)` rows for `sender_id`.
    async fn list_soft_deleted_facts(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String)>, MemoryError>;

    // --- scheduled tasks ---

    /// Insert a new scheduled task. Returns the new task id.
    #[allow(clippy::too_many_arguments)]
    async fn create_task(
        &self,
        channel: &str,
        sender_id: &str,
        reply_target: &str,
        description: &str,
        due_at: &str,
        repeat: Option<&str>,
        task_type: &str,
        project: &str,
    ) -> Result<String, MemoryError>;

    /// Pending tasks for `sender_id` as raw `(id, description, due_at,
    /// repeat, task_type, project)` rows, ordered by `due_at` ascending.
    async fn get_tasks_for_sender(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String, Option<String>, String, String)>, MemoryError>;

    /// Mark a task as completed. With `Some("daily")` / `Some("weekly")` /
    /// etc., reschedules the next occurrence; with `None` or `Some("once")`,
    /// the task transitions to a terminal status.
    async fn complete_task(&self, id: &str, repeat: Option<&str>) -> Result<(), MemoryError>;

    /// Record a task failure. Increments retry counter; transitions to a
    /// terminal failed status when retries exhaust. Returns `true` if the
    /// task transitioned to a terminal state.
    async fn fail_task(&self, id: &str, error: &str, max_retries: u32)
        -> Result<bool, MemoryError>;

    /// Cancel a pending task whose id starts with `id_prefix`, scoped to
    /// `sender_id`. Returns `true` if a row was cancelled.
    async fn cancel_task(&self, id_prefix: &str, sender_id: &str) -> Result<bool, MemoryError>;

    /// All pending tasks whose `due_at` is in the past.
    async fn get_due_tasks(&self) -> Result<Vec<DueTask>, MemoryError>;
}

#[async_trait]
impl MemoryStore for Store {
    async fn close_current_conversation(
        &self,
        channel: &str,
        sender_id: &str,
        project: &str,
    ) -> Result<bool, MemoryError> {
        Store::close_current_conversation(self, channel, sender_id, project).await
    }

    async fn get_memory_stats(&self, sender_id: &str) -> Result<(i64, i64, i64), MemoryError> {
        Store::get_memory_stats(self, sender_id).await
    }

    async fn db_size(&self) -> Result<u64, MemoryError> {
        Store::db_size(self).await
    }

    async fn get_total_usage(&self) -> Result<UsageSummary, MemoryError> {
        Store::get_total_usage(self).await
    }

    async fn get_history(
        &self,
        channel: &str,
        sender_id: &str,
        limit: i64,
    ) -> Result<Vec<HistoryRow>, MemoryError> {
        Store::get_history(self, channel, sender_id, limit).await
    }

    async fn search_messages(
        &self,
        query: &str,
        exclude_conversation_id: &str,
        sender_id: &str,
        limit: i64,
        since: Option<SystemTime>,
    ) -> Result<Vec<MessageRow>, MemoryError> {
        Store::search_messages(
            self,
            query,
            exclude_conversation_id,
            sender_id,
            limit,
            since,
        )
        .await
    }

    async fn get_message_by_id(&self, id: &str) -> Result<Option<MessageRow>, MemoryError> {
        Store::get_message_by_id(self, id).await
    }

    async fn store_fact(&self, sender_id: &str, key: &str, value: &str) -> Result<(), MemoryError> {
        Store::store_fact(self, sender_id, key, value).await
    }

    async fn get_fact(&self, sender_id: &str, key: &str) -> Result<Option<String>, MemoryError> {
        Store::get_fact(self, sender_id, key).await
    }

    async fn get_facts(&self, sender_id: &str) -> Result<Vec<(String, String)>, MemoryError> {
        Store::get_facts(self, sender_id).await
    }

    async fn soft_delete_fact(&self, sender_id: &str, key: &str) -> Result<bool, MemoryError> {
        Store::soft_delete_fact(self, sender_id, key).await
    }

    async fn soft_delete_facts(
        &self,
        sender_id: &str,
        key: Option<&str>,
    ) -> Result<u64, MemoryError> {
        Store::soft_delete_facts(self, sender_id, key).await
    }

    async fn list_soft_deleted_facts(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        Store::list_soft_deleted_facts(self, sender_id).await
    }

    async fn create_task(
        &self,
        channel: &str,
        sender_id: &str,
        reply_target: &str,
        description: &str,
        due_at: &str,
        repeat: Option<&str>,
        task_type: &str,
        project: &str,
    ) -> Result<String, MemoryError> {
        Store::create_task(
            self,
            channel,
            sender_id,
            reply_target,
            description,
            due_at,
            repeat,
            task_type,
            project,
        )
        .await
    }

    async fn get_tasks_for_sender(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String, Option<String>, String, String)>, MemoryError> {
        Store::get_tasks_for_sender(self, sender_id).await
    }

    async fn complete_task(&self, id: &str, repeat: Option<&str>) -> Result<(), MemoryError> {
        Store::complete_task(self, id, repeat).await
    }

    async fn fail_task(
        &self,
        id: &str,
        error: &str,
        max_retries: u32,
    ) -> Result<bool, MemoryError> {
        Store::fail_task(self, id, error, max_retries).await
    }

    async fn cancel_task(&self, id_prefix: &str, sender_id: &str) -> Result<bool, MemoryError> {
        Store::cancel_task(self, id_prefix, sender_id).await
    }

    async fn get_due_tasks(&self) -> Result<Vec<DueTask>, MemoryError> {
        Store::get_due_tasks(self).await
    }
}

/// Expose a [`Store`] through the [`MemoryStore`] trait surface.
///
/// `Store` already implements `Clone` (its `SqlitePool` is internally
/// reference-counted); cloning here shares the same connection pool.
pub fn into_handle(store: Store) -> Arc<dyn MemoryStore> {
    Arc::new(store)
}
