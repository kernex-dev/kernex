//! Message storage and full-text search.

use std::time::SystemTime;

use super::Store;
use crate::error::MemoryError;
use crate::types::{format_sqlite_timestamp, parse_sqlite_timestamp, MessageRow};
use kernex_core::message::{Request, Response};
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
    ) -> Result<(), MemoryError> {
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
        .map_err(|e| MemoryError::sqlite("insert failed", e))?;

        // Store assistant response.
        let asst_id = Uuid::new_v4().to_string();
        let metadata_json = serde_json::to_string(&response.metadata)
            .map_err(|e| MemoryError::serde("serialize failed", e))?;

        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content, metadata_json) VALUES (?, ?, 'assistant', ?, ?)",
        )
        .bind(&asst_id)
        .bind(&conv_id)
        .bind(&response.text)
        .bind(&metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("insert failed", e))?;

        Ok(())
    }

    /// Fetch a single message row by its UUID. Returns `None` when the
    /// id is missing. The `MemoryStore` trait method
    /// `get_message_by_id` delegates here.
    pub async fn get_message_by_id(&self, id: &str) -> Result<Option<MessageRow>, MemoryError> {
        let row: Option<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, conversation_id, role, content, timestamp \
             FROM messages WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("get_message_by_id failed", e))?;

        match row {
            Some((id, conversation_id, role, content, timestamp)) => Ok(Some(MessageRow {
                id,
                conversation_id,
                role,
                content,
                timestamp: parse_sqlite_timestamp(&timestamp)?,
            })),
            None => Ok(None),
        }
    }

    /// Search past messages across all conversations using FTS5 full-text search.
    /// Honors an optional `since` recency cutoff: when `Some`, only
    /// messages with `timestamp >= since` are returned, and `limit`
    /// applies after the filter (resolves the S-search-2 ambiguity
    /// flagged in the kx-mem-cli-promotion spec).
    pub async fn search_messages(
        &self,
        query: &str,
        exclude_conversation_id: &str,
        sender_id: &str,
        limit: i64,
        since: Option<SystemTime>,
    ) -> Result<Vec<MessageRow>, MemoryError> {
        if query.len() < 3 {
            return Ok(Vec::new());
        }

        // Wrap in double quotes and escape internal quotes to prevent FTS5 operator
        // injection (AND, OR, NOT, NEAR, *, etc.).
        let sanitized = format!("\"{}\"", query.replace('"', "\"\""));

        // Build the SQL with an optional `since` predicate. Using a
        // branch instead of a single conditional SQL string keeps the
        // prepared statement shape stable for sqlx's cache.
        let rows: Vec<(String, String, String, String, String)> = if let Some(cutoff) = since {
            let cutoff_str = format_sqlite_timestamp(cutoff);
            sqlx::query_as(
                "SELECT m.id, m.conversation_id, m.role, m.content, m.timestamp \
                 FROM messages_fts fts \
                 JOIN messages m ON m.rowid = fts.rowid \
                 JOIN conversations c ON c.id = m.conversation_id \
                 WHERE messages_fts MATCH ? \
                 AND m.conversation_id != ? \
                 AND c.sender_id = ? \
                 AND m.timestamp >= ? \
                 ORDER BY rank \
                 LIMIT ?",
            )
            .bind(&sanitized)
            .bind(exclude_conversation_id)
            .bind(sender_id)
            .bind(&cutoff_str)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(
                "SELECT m.id, m.conversation_id, m.role, m.content, m.timestamp \
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
        }
        .map_err(|e| MemoryError::sqlite("fts search failed", e))?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, conversation_id, role, content, timestamp) in rows {
            out.push(MessageRow {
                id,
                conversation_id,
                role,
                content,
                timestamp: parse_sqlite_timestamp(&timestamp)?,
            });
        }
        Ok(out)
    }
}
