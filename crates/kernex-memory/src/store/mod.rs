//! SQLite-backed persistent memory store.
//!
//! Split into focused submodules:
//! - `conversations` — conversation lifecycle (create, find, close, summaries)
//! - `messages` — message storage and full-text search
//! - `facts` — user facts, aliases, and limitations
//! - `tasks` — scheduled task CRUD and dedup
//! - `context` — context building and user profile formatting
//! - `context_helpers` — onboarding stages, system prompt composition, language detection

mod context;
mod context_helpers;
mod conversations;
mod facts;
mod messages;
mod outcomes;
mod sessions;
mod tasks;
mod usage;

pub use context::{detect_language, format_user_profile};
pub use tasks::DueTask;
pub use usage::UsageSummary;

use kernex_core::{config::MemoryConfig, error::KernexError, shellexpand};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;
use tracing::info;

/// How long (in minutes) before a conversation is considered idle.
const CONVERSATION_TIMEOUT_MINUTES: i64 = 120;

/// Persistent memory store backed by SQLite.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    max_context_messages: usize,
}

impl Store {
    /// Create a new store, running migrations on first use.
    pub async fn new(config: &MemoryConfig) -> Result<Self, KernexError> {
        let db_path = shellexpand(&config.db_path);

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| KernexError::Store(format!("failed to create data dir: {e}")))?;
        }

        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))
            .map_err(|e| KernexError::Store(format!("invalid db path: {e}")))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .map_err(|e| KernexError::Store(format!("failed to connect to sqlite: {e}")))?;

        Self::run_migrations(&pool).await?;

        info!("Memory store initialized at {db_path}");

        Ok(Self {
            pool,
            max_context_messages: config.max_context_messages,
        })
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get the database file size in bytes.
    pub async fn db_size(&self) -> Result<u64, KernexError> {
        let (page_count,): (i64,) = sqlx::query_as("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("pragma failed: {e}")))?;

        let (page_size,): (i64,) = sqlx::query_as("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("pragma failed: {e}")))?;

        Ok((page_count * page_size) as u64)
    }

    /// Run SQL migrations, tracking which have already been applied.
    pub(crate) async fn run_migrations(pool: &SqlitePool) -> Result<(), KernexError> {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .execute(pool)
        .await
        .map_err(|e| KernexError::Store(format!("failed to create migrations table: {e}")))?;

        // Bootstrap: if _migrations is empty but tables already exist from
        // a pre-tracking era, mark all existing migrations as applied.
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _migrations")
            .fetch_one(pool)
            .await
            .map_err(|e| KernexError::Store(format!("failed to count migrations: {e}")))?;

        if count.0 == 0 {
            let has_summary: bool = sqlx::query_scalar::<_, String>(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='conversations'",
            )
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .map(|sql| sql.contains("summary"))
            .unwrap_or(false);

            if has_summary {
                for name in &["001_init", "002_audit_log", "003_memory_enhancement"] {
                    sqlx::query("INSERT OR IGNORE INTO _migrations (name) VALUES (?)")
                        .bind(name)
                        .execute(pool)
                        .await
                        .map_err(|e| {
                            KernexError::Store(format!("failed to bootstrap migration {name}: {e}"))
                        })?;
                }
            }
        }

        let migrations: &[(&str, &str)] = &[
            ("001_init", include_str!("../../migrations/001_init.sql")),
            (
                "002_audit_log",
                include_str!("../../migrations/002_audit_log.sql"),
            ),
            (
                "003_memory_enhancement",
                include_str!("../../migrations/003_memory_enhancement.sql"),
            ),
            (
                "004_fts5_recall",
                include_str!("../../migrations/004_fts5_recall.sql"),
            ),
            (
                "005_scheduled_tasks",
                include_str!("../../migrations/005_scheduled_tasks.sql"),
            ),
            (
                "006_limitations",
                include_str!("../../migrations/006_limitations.sql"),
            ),
            (
                "007_task_type",
                include_str!("../../migrations/007_task_type.sql"),
            ),
            (
                "008_user_aliases",
                include_str!("../../migrations/008_user_aliases.sql"),
            ),
            (
                "009_task_retry",
                include_str!("../../migrations/009_task_retry.sql"),
            ),
            (
                "010_outcomes",
                include_str!("../../migrations/010_outcomes.sql"),
            ),
            (
                "011_project_learning",
                include_str!("../../migrations/011_project_learning.sql"),
            ),
            (
                "012_project_sessions",
                include_str!("../../migrations/012_project_sessions.sql"),
            ),
            (
                "013_multi_lessons",
                include_str!("../../migrations/013_multi_lessons.sql"),
            ),
            (
                "014_token_usage",
                include_str!("../../migrations/014_token_usage.sql"),
            ),
        ];

        for (name, sql) in migrations {
            let applied: Option<(String,)> =
                sqlx::query_as("SELECT name FROM _migrations WHERE name = ?")
                    .bind(name)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| {
                        KernexError::Store(format!("failed to check migration {name}: {e}"))
                    })?;

            if applied.is_some() {
                continue;
            }

            sqlx::raw_sql(sql)
                .execute(pool)
                .await
                .map_err(|e| KernexError::Store(format!("migration {name} failed: {e}")))?;

            sqlx::query("INSERT INTO _migrations (name) VALUES (?)")
                .bind(name)
                .execute(pool)
                .await
                .map_err(|e| {
                    KernexError::Store(format!("failed to record migration {name}: {e}"))
                })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
