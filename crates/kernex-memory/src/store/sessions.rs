//! Project-scoped CLI session persistence.
//!
//! SQLite-backed sessions scoped per (channel, sender_id, project).

use super::Store;
use kernex_core::error::KernexError;
use uuid::Uuid;

impl Store {
    /// Upsert a CLI session for a (channel, sender_id, project) tuple.
    pub async fn store_session(
        &self,
        channel: &str,
        sender_id: &str,
        project: &str,
        session_id: &str,
    ) -> Result<(), KernexError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO project_sessions (id, channel, sender_id, project, session_id) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(channel, sender_id, project) \
             DO UPDATE SET session_id = excluded.session_id, updated_at = datetime('now')",
        )
        .bind(&id)
        .bind(channel)
        .bind(sender_id)
        .bind(project)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("store_session failed: {e}")))?;

        Ok(())
    }

    /// Look up the CLI session_id for a (channel, sender_id, project) tuple.
    pub async fn get_session(
        &self,
        channel: &str,
        sender_id: &str,
        project: &str,
    ) -> Result<Option<String>, KernexError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT session_id FROM project_sessions \
             WHERE channel = ? AND sender_id = ? AND project = ?",
        )
        .bind(channel)
        .bind(sender_id)
        .bind(project)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get_session failed: {e}")))?;

        Ok(row.map(|(sid,)| sid))
    }

    /// Delete the CLI session for a specific (channel, sender_id, project).
    pub async fn clear_session(
        &self,
        channel: &str,
        sender_id: &str,
        project: &str,
    ) -> Result<(), KernexError> {
        sqlx::query(
            "DELETE FROM project_sessions \
             WHERE channel = ? AND sender_id = ? AND project = ?",
        )
        .bind(channel)
        .bind(sender_id)
        .bind(project)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("clear_session failed: {e}")))?;

        Ok(())
    }

    /// Delete all CLI sessions for a sender.
    pub async fn clear_all_sessions_for_sender(&self, sender_id: &str) -> Result<(), KernexError> {
        sqlx::query("DELETE FROM project_sessions WHERE sender_id = ?")
            .bind(sender_id)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("clear_all_sessions failed: {e}")))?;

        Ok(())
    }
}
