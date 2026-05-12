//! Typed row shapes returned by the `MemoryStore` trait.
//!
//! Replaces the prior `(String, String, String)` tuples on
//! `search_messages` and the `(String, String)` tuples on `get_history`
//! so consumers never see raw SQLite timestamp strings. `timestamp` and
//! `updated_at` are parsed at fetch time into `SystemTime`; consumers
//! compare and format from there.

use std::time::SystemTime;

use crate::error::MemoryError;

/// A single message row, returned by `search_messages` and
/// `get_message_by_id`. `timestamp` is parsed from the SQLite TIMESTAMP
/// column at fetch time so consumers compare against `SystemTime`
/// directly instead of parsing a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    /// Stable UUID identifying the message row.
    pub id: String,
    /// Identifier of the conversation the message belongs to.
    pub conversation_id: String,
    /// `"user"` or `"assistant"`.
    pub role: String,
    /// Full message body text.
    pub content: String,
    /// Wall-clock time the message was stored, in UTC.
    pub timestamp: SystemTime,
}

/// One closed-conversation summary row returned by `get_history`.
/// `updated_at` is parsed from SQLite at fetch time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    /// Identifier of the conversation.
    pub conversation_id: String,
    /// Conversation summary text; `"(no summary)"` when the column is
    /// NULL, matching the inherent-method fallback shipped before the
    /// typed-row trait surface landed.
    pub summary: String,
    /// Wall-clock time the conversation was last updated, in UTC.
    pub updated_at: SystemTime,
}

/// Parse a SQLite `TIMESTAMP` string ("YYYY-MM-DD HH:MM:SS", UTC) into a
/// `SystemTime`. Returns `MemoryError::Logic` on shape mismatch so
/// callers can distinguish a parse failure from a database error.
///
/// The schema's `timestamp` and `updated_at` columns default to
/// `datetime('now')`, which always emits the 19-character format above.
/// A failure here means the column was hand-edited or the schema
/// drifted; in either case it is a logic/data error, not a SQLite error.
pub(crate) fn parse_sqlite_timestamp(s: &str) -> Result<SystemTime, MemoryError> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| MemoryError::logic(format!("parse timestamp {s:?}: {e}")))?;
    Ok(naive.and_utc().into())
}

/// Format a `SystemTime` back to the SQLite `TIMESTAMP` shape
/// ("YYYY-MM-DD HH:MM:SS", UTC). Used by `search_messages` when binding
/// the `since` parameter to the prepared statement.
pub(crate) fn format_sqlite_timestamp(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_roundtrips_through_format() {
        let original = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        let formatted = format_sqlite_timestamp(original);
        let parsed = parse_sqlite_timestamp(&formatted).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = parse_sqlite_timestamp("not a timestamp").unwrap_err();
        assert!(matches!(err, MemoryError::Logic(_)));
    }

    #[test]
    fn parse_accepts_schema_default_shape() {
        // datetime('now') emits "YYYY-MM-DD HH:MM:SS" without timezone.
        let parsed = parse_sqlite_timestamp("2026-05-11 18:00:00").unwrap();
        let formatted = format_sqlite_timestamp(parsed);
        assert_eq!(formatted, "2026-05-11 18:00:00");
    }
}
