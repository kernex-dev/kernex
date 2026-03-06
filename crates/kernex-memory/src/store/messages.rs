//! Message storage and full-text search.

use super::Store;
use kernex_core::{
    error::KernexError,
    message::{Request, Response},
};
use uuid::Uuid;

impl Store {
    /// Store a user message and assistant response.
    ///
    /// The `channel` parameter identifies the communication channel (e.g. "api",
    /// "slack") since `Request` is channel-agnostic.
    pub async fn store_exchange(
        &self,
        channel: &str,
        incoming: &Request,
        response: &Response,
        project: &str,
    ) -> Result<(), KernexError> {
        let conv_id = self
            .get_or_create_conversation(channel, &incoming.sender_id, project)
            .await?;

        // Store user message.
        let user_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content) VALUES (?, ?, 'user', ?)",
        )
        .bind(&user_id)
        .bind(&conv_id)
        .bind(&incoming.text)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("insert failed: {e}")))?;

        // Store assistant response.
        let asst_id = Uuid::new_v4().to_string();
        let metadata_json = serde_json::to_string(&response.metadata)
            .map_err(|e| KernexError::Store(format!("serialize failed: {e}")))?;

        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content, metadata_json) VALUES (?, ?, 'assistant', ?, ?)",
        )
        .bind(&asst_id)
        .bind(&conv_id)
        .bind(&response.text)
        .bind(&metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("insert failed: {e}")))?;

        Ok(())
    }

    /// Search past messages across all conversations using FTS5 full-text search.
    pub async fn search_messages(
        &self,
        query: &str,
        exclude_conversation_id: &str,
        sender_id: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, String)>, KernexError> {
        if query.len() < 3 {
            return Ok(Vec::new());
        }

        // Wrap in double quotes and escape internal quotes to prevent FTS5 operator
        // injection (AND, OR, NOT, NEAR, *, etc.).
        let sanitized = format!("\"{}\"", query.replace('"', "\"\""));

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT m.role, m.content, m.timestamp \
             FROM messages_fts fts \
             JOIN messages m ON m.rowid = fts.rowid \
             JOIN conversations c ON c.id = m.conversation_id \
             WHERE messages_fts MATCH ? \
             AND m.conversation_id != ? \
             AND c.sender_id = ? \
             ORDER BY rank \
             LIMIT ?",
        )
        .bind(&sanitized)
        .bind(exclude_conversation_id)
        .bind(sender_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("fts search failed: {e}")))?;

        Ok(rows)
    }
}
