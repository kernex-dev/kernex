//! Audit log — records every interaction through Kernex.

use kernex_core::error::KernexError;
use sqlx::SqlitePool;
use tracing::debug;
use uuid::Uuid;

/// An entry to write to the audit log.
pub struct AuditEntry {
    pub channel: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub input_text: String,
    pub output_text: Option<String>,
    pub provider_used: Option<String>,
    pub model: Option<String>,
    pub processing_ms: Option<i64>,
    pub status: AuditStatus,
    pub denial_reason: Option<String>,
}

/// Status of an audited interaction.
pub enum AuditStatus {
    Ok,
    Error,
    Denied,
}

impl AuditStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
        }
    }
}

/// Audit logger backed by SQLite.
#[derive(Clone)]
pub struct AuditLogger {
    pool: SqlitePool,
}

impl AuditLogger {
    /// Create a new audit logger sharing the given pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Write an entry to the audit log.
    pub async fn log(&self, entry: &AuditEntry) -> Result<(), KernexError> {
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO audit_log \
             (id, channel, sender_id, sender_name, input_text, output_text, \
              provider_used, model, processing_ms, status, denial_reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&entry.channel)
        .bind(&entry.sender_id)
        .bind(&entry.sender_name)
        .bind(&entry.input_text)
        .bind(&entry.output_text)
        .bind(&entry.provider_used)
        .bind(&entry.model)
        .bind(entry.processing_ms)
        .bind(entry.status.as_str())
        .bind(&entry.denial_reason)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("audit log write failed: {e}")))?;

        debug!(
            "audit: {} {} [{}] {}",
            entry.channel,
            entry.sender_id,
            entry.status.as_str(),
            truncate(&entry.input_text, 80)
        );

        Ok(())
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::Row;
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/002_audit_log.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn test_audit_logger_log_inserts_entry() {
        let pool = test_pool().await;
        let logger = AuditLogger::new(pool.clone());

        let entry = AuditEntry {
            channel: "api".to_string(),
            sender_id: "user42".to_string(),
            sender_name: Some("Alice".to_string()),
            input_text: "hello kernex".to_string(),
            output_text: Some("hi there".to_string()),
            provider_used: Some("claude-code".to_string()),
            model: Some("sonnet".to_string()),
            processing_ms: Some(123),
            status: AuditStatus::Ok,
            denial_reason: None,
        };

        logger.log(&entry).await.unwrap();

        let row = sqlx::query("SELECT channel, sender_id, sender_name, input_text, output_text, provider_used, model, processing_ms, status, denial_reason FROM audit_log LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(row.get::<String, _>("channel"), "api");
        assert_eq!(row.get::<String, _>("sender_id"), "user42");
        assert_eq!(
            row.get::<Option<String>, _>("sender_name"),
            Some("Alice".to_string())
        );
        assert_eq!(row.get::<String, _>("input_text"), "hello kernex");
        assert_eq!(
            row.get::<Option<String>, _>("output_text"),
            Some("hi there".to_string())
        );
        assert_eq!(
            row.get::<Option<String>, _>("provider_used"),
            Some("claude-code".to_string())
        );
        assert_eq!(
            row.get::<Option<String>, _>("model"),
            Some("sonnet".to_string())
        );
        assert_eq!(row.get::<Option<i64>, _>("processing_ms"), Some(123));
        assert_eq!(row.get::<String, _>("status"), "ok");
        assert_eq!(
            row.get::<Option<String>, _>("denial_reason"),
            None::<String>
        );
    }

    #[test]
    fn test_truncate_ascii() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_multibyte() {
        let s = "\u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{043c}\u{0438}\u{0440}!";
        let result = truncate(s, 5);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_truncate_emoji() {
        let s = "Hi \u{1f389} there";
        let result = truncate(s, 4);
        assert!(!result.is_empty());
    }
}
