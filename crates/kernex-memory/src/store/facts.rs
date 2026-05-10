//! User facts, cross-channel aliases, and limitations.

use super::Store;
use crate::error::MemoryError;
use uuid::Uuid;

impl Store {
    /// Store a fact (upsert by sender_id + key). If the row was previously
    /// soft-deleted, re-storing clears `deleted_at` so the value is visible
    /// again to default-filtered reads.
    pub async fn store_fact(
        &self,
        sender_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), MemoryError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO facts (id, sender_id, key, value) VALUES (?, ?, ?, ?) \
             ON CONFLICT(sender_id, key) DO UPDATE SET \
                value = excluded.value, \
                updated_at = datetime('now'), \
                deleted_at = NULL",
        )
        .bind(&id)
        .bind(sender_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("upsert fact failed", e))?;

        Ok(())
    }

    /// Get a single fact by sender and key. Returns `None` if the row is
    /// soft-deleted.
    pub async fn get_fact(
        &self,
        sender_id: &str,
        key: &str,
    ) -> Result<Option<String>, MemoryError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM facts \
             WHERE sender_id = ? AND key = ? AND deleted_at IS NULL",
        )
        .bind(sender_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(row.map(|(v,)| v))
    }

    /// Hard-delete a single fact by sender and key. Returns `true` if a row
    /// was deleted. Emergency cleanup only — not exposed on the
    /// `MemoryStore` trait. Default consumer paths should call
    /// [`Self::soft_delete_fact`].
    pub async fn delete_fact(&self, sender_id: &str, key: &str) -> Result<bool, MemoryError> {
        let result = sqlx::query("DELETE FROM facts WHERE sender_id = ? AND key = ?")
            .bind(sender_id)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("delete failed", e))?;

        Ok(result.rows_affected() > 0)
    }

    /// Soft-delete a single fact by setting its `deleted_at` timestamp.
    /// Returns `true` if a row transitioned from active to deleted.
    pub async fn soft_delete_fact(&self, sender_id: &str, key: &str) -> Result<bool, MemoryError> {
        let result = sqlx::query(
            "UPDATE facts SET deleted_at = datetime('now') \
             WHERE sender_id = ? AND key = ? AND deleted_at IS NULL",
        )
        .bind(sender_id)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("soft delete failed", e))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get all active (not soft-deleted) facts for a sender.
    pub async fn get_facts(&self, sender_id: &str) -> Result<Vec<(String, String)>, MemoryError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM facts \
             WHERE sender_id = ? AND deleted_at IS NULL ORDER BY key",
        )
        .bind(sender_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(rows)
    }

    /// Hard-delete facts for a sender — all if `key` is `None`, specific
    /// fact if `key` is `Some`. Emergency cleanup only — not exposed on the
    /// `MemoryStore` trait. Default consumer paths should call
    /// [`Self::soft_delete_facts`].
    pub async fn delete_facts(
        &self,
        sender_id: &str,
        key: Option<&str>,
    ) -> Result<u64, MemoryError> {
        let result = if let Some(k) = key {
            sqlx::query("DELETE FROM facts WHERE sender_id = ? AND key = ?")
                .bind(sender_id)
                .bind(k)
                .execute(&self.pool)
                .await
        } else {
            sqlx::query("DELETE FROM facts WHERE sender_id = ?")
                .bind(sender_id)
                .execute(&self.pool)
                .await
        };

        result
            .map(|r| r.rows_affected())
            .map_err(|e| MemoryError::sqlite("delete failed", e))
    }

    /// Soft-delete facts for a sender — every active fact if `key` is
    /// `None`, only the matching active fact if `key` is `Some`. Returns
    /// the count of rows that transitioned from active to deleted.
    pub async fn soft_delete_facts(
        &self,
        sender_id: &str,
        key: Option<&str>,
    ) -> Result<u64, MemoryError> {
        let result = if let Some(k) = key {
            sqlx::query(
                "UPDATE facts SET deleted_at = datetime('now') \
                 WHERE sender_id = ? AND key = ? AND deleted_at IS NULL",
            )
            .bind(sender_id)
            .bind(k)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE facts SET deleted_at = datetime('now') \
                 WHERE sender_id = ? AND deleted_at IS NULL",
            )
            .bind(sender_id)
            .execute(&self.pool)
            .await
        };

        result
            .map(|r| r.rows_affected())
            .map_err(|e| MemoryError::sqlite("soft delete failed", e))
    }

    /// Read soft-deleted facts for a sender (debug / recovery helper).
    /// Returns `(key, value, deleted_at)` rows ordered by deletion time
    /// descending.
    pub async fn list_soft_deleted_facts(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT key, value, deleted_at FROM facts \
             WHERE sender_id = ? AND deleted_at IS NOT NULL \
             ORDER BY deleted_at DESC",
        )
        .bind(sender_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(rows)
    }

    /// Get all active facts across all users (excluding the `welcomed`
    /// marker key).
    pub async fn get_all_facts(&self) -> Result<Vec<(String, String)>, MemoryError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM facts \
             WHERE key != 'welcomed' AND deleted_at IS NULL ORDER BY key",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(rows)
    }

    /// Get all active `(sender_id, value)` pairs for a given fact key
    /// across all users.
    pub async fn get_all_facts_by_key(
        &self,
        key: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT sender_id, value FROM facts \
             WHERE key = ? AND deleted_at IS NULL ORDER BY sender_id",
        )
        .bind(key)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("get facts by key failed", e))?;
        Ok(rows)
    }

    /// Check if a sender has never been welcomed (no active `welcomed`
    /// fact).
    pub async fn is_new_user(&self, sender_id: &str) -> Result<bool, MemoryError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM facts \
             WHERE sender_id = ? AND key = 'welcomed' AND deleted_at IS NULL",
        )
        .bind(sender_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(row.is_none())
    }

    // --- Aliases ---

    /// Resolve a sender_id to its canonical form via the user_aliases table.
    pub async fn resolve_sender_id(&self, sender_id: &str) -> Result<String, MemoryError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT canonical_sender_id FROM user_aliases WHERE alias_sender_id = ?",
        )
        .bind(sender_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("resolve alias failed", e))?;

        Ok(row.map(|(id,)| id).unwrap_or_else(|| sender_id.to_string()))
    }

    /// Create an alias mapping: alias_id → canonical_id.
    pub async fn create_alias(
        &self,
        alias_id: &str,
        canonical_id: &str,
    ) -> Result<(), MemoryError> {
        sqlx::query(
            "INSERT OR IGNORE INTO user_aliases (alias_sender_id, canonical_sender_id) \
             VALUES (?, ?)",
        )
        .bind(alias_id)
        .bind(canonical_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("create alias failed", e))?;

        Ok(())
    }

    /// Find an existing welcomed user different from `sender_id`. Skips
    /// soft-deleted `welcomed` markers.
    pub async fn find_canonical_user(
        &self,
        exclude_sender_id: &str,
    ) -> Result<Option<String>, MemoryError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT sender_id FROM facts \
             WHERE key = 'welcomed' AND sender_id != ? AND deleted_at IS NULL LIMIT 1",
        )
        .bind(exclude_sender_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("query failed", e))?;

        Ok(row.map(|(id,)| id))
    }

    // --- Limitations ---

    /// Store a limitation (deduplicates by title, case-insensitive).
    pub async fn store_limitation(
        &self,
        title: &str,
        description: &str,
        proposed_plan: &str,
    ) -> Result<bool, MemoryError> {
        let id = Uuid::new_v4().to_string();
        let result = sqlx::query(
            "INSERT OR IGNORE INTO limitations (id, title, description, proposed_plan) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(title)
        .bind(description)
        .bind(proposed_plan)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("store limitation failed", e))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get all open limitations: (title, description, proposed_plan).
    pub async fn get_open_limitations(&self) -> Result<Vec<(String, String, String)>, MemoryError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT title, description, proposed_plan FROM limitations \
             WHERE status = 'open' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("get open limitations failed", e))?;

        Ok(rows)
    }
}
