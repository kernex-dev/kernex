//! User facts, cross-channel aliases, and limitations.

use super::Store;
use kernex_core::error::KernexError;
use uuid::Uuid;

impl Store {
    /// Store a fact (upsert by sender_id + key).
    pub async fn store_fact(
        &self,
        sender_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), KernexError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO facts (id, sender_id, key, value) VALUES (?, ?, ?, ?) \
             ON CONFLICT(sender_id, key) DO UPDATE SET value = excluded.value, updated_at = datetime('now')",
        )
        .bind(&id)
        .bind(sender_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("upsert fact failed: {e}")))?;

        Ok(())
    }

    /// Get a single fact by sender and key.
    pub async fn get_fact(
        &self,
        sender_id: &str,
        key: &str,
    ) -> Result<Option<String>, KernexError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM facts WHERE sender_id = ? AND key = ?")
                .bind(sender_id)
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

        Ok(row.map(|(v,)| v))
    }

    /// Delete a single fact by sender and key. Returns `true` if a row was deleted.
    pub async fn delete_fact(&self, sender_id: &str, key: &str) -> Result<bool, KernexError> {
        let result = sqlx::query("DELETE FROM facts WHERE sender_id = ? AND key = ?")
            .bind(sender_id)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("delete failed: {e}")))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get all facts for a sender.
    pub async fn get_facts(&self, sender_id: &str) -> Result<Vec<(String, String)>, KernexError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM facts WHERE sender_id = ? ORDER BY key")
                .bind(sender_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

        Ok(rows)
    }

    /// Delete facts for a sender — all facts if key is None, specific fact if key is Some.
    pub async fn delete_facts(
        &self,
        sender_id: &str,
        key: Option<&str>,
    ) -> Result<u64, KernexError> {
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
            .map_err(|e| KernexError::Store(format!("delete failed: {e}")))
    }

    /// Get all facts across all users.
    pub async fn get_all_facts(&self) -> Result<Vec<(String, String)>, KernexError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM facts WHERE key != 'welcomed' ORDER BY key")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

        Ok(rows)
    }

    /// Get all `(sender_id, value)` pairs for a given fact key across all users.
    pub async fn get_all_facts_by_key(
        &self,
        key: &str,
    ) -> Result<Vec<(String, String)>, KernexError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT sender_id, value FROM facts WHERE key = ? ORDER BY sender_id")
                .bind(key)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("get facts by key failed: {e}")))?;
        Ok(rows)
    }

    /// Check if a sender has never been welcomed (no `welcomed` fact).
    pub async fn is_new_user(&self, sender_id: &str) -> Result<bool, KernexError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM facts WHERE sender_id = ? AND key = 'welcomed'")
                .bind(sender_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

        Ok(row.is_none())
    }

    // --- Aliases ---

    /// Resolve a sender_id to its canonical form via the user_aliases table.
    pub async fn resolve_sender_id(&self, sender_id: &str) -> Result<String, KernexError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT canonical_sender_id FROM user_aliases WHERE alias_sender_id = ?",
        )
        .bind(sender_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("resolve alias failed: {e}")))?;

        Ok(row.map(|(id,)| id).unwrap_or_else(|| sender_id.to_string()))
    }

    /// Create an alias mapping: alias_id → canonical_id.
    pub async fn create_alias(
        &self,
        alias_id: &str,
        canonical_id: &str,
    ) -> Result<(), KernexError> {
        sqlx::query(
            "INSERT OR IGNORE INTO user_aliases (alias_sender_id, canonical_sender_id) \
             VALUES (?, ?)",
        )
        .bind(alias_id)
        .bind(canonical_id)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("create alias failed: {e}")))?;

        Ok(())
    }

    /// Find an existing welcomed user different from `sender_id`.
    pub async fn find_canonical_user(
        &self,
        exclude_sender_id: &str,
    ) -> Result<Option<String>, KernexError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT sender_id FROM facts WHERE key = 'welcomed' AND sender_id != ? LIMIT 1",
        )
        .bind(exclude_sender_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

        Ok(row.map(|(id,)| id))
    }

    // --- Limitations ---

    /// Store a limitation (deduplicates by title, case-insensitive).
    pub async fn store_limitation(
        &self,
        title: &str,
        description: &str,
        proposed_plan: &str,
    ) -> Result<bool, KernexError> {
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
        .map_err(|e| KernexError::Store(format!("store limitation failed: {e}")))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get all open limitations: (title, description, proposed_plan).
    pub async fn get_open_limitations(&self) -> Result<Vec<(String, String, String)>, KernexError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT title, description, proposed_plan FROM limitations \
             WHERE status = 'open' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get open limitations failed: {e}")))?;

        Ok(rows)
    }
}
