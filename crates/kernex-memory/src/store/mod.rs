//! SQLite-backed persistent memory store.
//!
//! Split into focused submodules:
//! - `conversations` — conversation lifecycle (create, find, close, summaries)
//! - `messages` — message storage and full-text search
//! - `facts` — user facts, aliases, and limitations
//! - `tasks` — scheduled task CRUD and dedup
//! - `context` — context building and user profile formatting
//! - `context_helpers` — onboarding stages, system prompt composition, language detection

mod checkpoints;
mod context;
mod context_helpers;
mod conversations;
mod facts;
mod messages;
mod observations;
mod outcomes;
mod sessions;
mod tasks;
mod usage;

pub use checkpoints::PhaseCheckpoint;
pub use context::{detect_language, format_user_profile};
pub use tasks::{DueTask, TaskRunRecord};
pub use usage::{UsageBreakdown, UsageSummary};

use crate::error::MemoryError;
use kernex_core::{config::MemoryConfig, shellexpand};
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
    pub async fn new(config: &MemoryConfig) -> Result<Self, MemoryError> {
        let db_path = shellexpand(&config.db_path);

        // Ensure parent directory exists. On Unix, also restrict its mode to
        // 0o700 so other local users can't enumerate or read the SQLite WAL
        // files that may briefly contain message text en route to disk.
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MemoryError::io("failed to create data dir", e))?;
            tighten_unix_dir_perms(parent);
        }

        // Pre-create the SQLite file with mode 0o600 *before* sqlx connects.
        // Otherwise sqlx's create_if_missing path creates the file under the
        // process umask (typically 0o644) and our chmod runs only after
        // migrations — leaving a window where another local user can read
        // messages, facts, and audit log entries on a shared host.
        precreate_sqlite_file(&db_path)?;

        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))
            .map_err(|e| MemoryError::sqlite("invalid db path", e))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(opts)
            .await
            .map_err(|e| MemoryError::sqlite("failed to connect to sqlite", e))?;

        Self::run_migrations(&pool).await?;

        // Belt-and-braces: re-tighten in case sqlx (or a future version of
        // it) ever recreates the file under a relaxed mode.
        tighten_unix_file_perms(&db_path);

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
}

/// Create the SQLite file at `db_path` with mode 0o600 if it doesn't already
/// exist. `:memory:` and any non-disk URI is left alone. On non-Unix this is
/// a best-effort `create_new` without explicit mode bits.
fn precreate_sqlite_file(db_path: &str) -> Result<(), MemoryError> {
    if db_path == ":memory:" || db_path.starts_with("file::memory:") {
        return Ok(());
    }
    let path = std::path::Path::new(db_path);
    if path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => Ok(()),
            // Race with another process — the file now exists, that's fine
            // because tighten_unix_file_perms runs after migrations.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(MemoryError::io("pre-create db file at 0o600", e)),
        }
    }
    #[cfg(not(unix))]
    {
        // On Windows ACLs are inherited from the parent dir; we have no
        // useful mode bits to set here. Touching the file early would just
        // duplicate what sqlx does.
        let _ = path;
        Ok(())
    }
}

#[cfg(unix)]
fn tighten_unix_file_perms(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        if let Err(e) = std::fs::set_permissions(path, perms) {
            tracing::warn!(path = %path, "could not chmod 0600 on memory db: {e}");
        }
    }
}

#[cfg(not(unix))]
fn tighten_unix_file_perms(_path: &str) {}

#[cfg(unix)]
fn tighten_unix_dir_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        if let Err(e) = std::fs::set_permissions(path, perms) {
            tracing::warn!(path = %path.display(), "could not chmod 0700 on memory data dir: {e}");
        }
    }
}

#[cfg(not(unix))]
fn tighten_unix_dir_perms(_path: &std::path::Path) {}

impl Store {
    /// Get the database file size in bytes.
    pub async fn db_size(&self) -> Result<u64, MemoryError> {
        let (page_count,): (i64,) = sqlx::query_as("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("pragma failed", e))?;

        let (page_size,): (i64,) = sqlx::query_as("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("pragma failed", e))?;

        Ok((page_count * page_size) as u64)
    }

    /// Run SQL migrations, tracking which have already been applied.
    pub(crate) async fn run_migrations(pool: &SqlitePool) -> Result<(), MemoryError> {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .execute(pool)
        .await
        .map_err(|e| MemoryError::sqlite("failed to create migrations table", e))?;

        // Bootstrap: if _migrations is empty but tables already exist from
        // a pre-tracking era, mark all existing migrations as applied.
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _migrations")
            .fetch_one(pool)
            .await
            .map_err(|e| MemoryError::sqlite("failed to count migrations", e))?;

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
                            MemoryError::sqlite(format!("failed to bootstrap migration {name}"), e)
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
            (
                "015_phase_checkpoints",
                include_str!("../../migrations/015_phase_checkpoints.sql"),
            ),
            (
                "016_cache_token_breakdown",
                include_str!("../../migrations/016_cache_token_breakdown.sql"),
            ),
            (
                "017_soft_delete",
                include_str!("../../migrations/017_soft_delete.sql"),
            ),
            (
                "018_observations",
                include_str!("../../migrations/018_observations.sql"),
            ),
            (
                "019_task_runs",
                include_str!("../../migrations/019_task_runs.sql"),
            ),
        ];

        // Fast-path: fetch the applied-migrations set in a single SELECT
        // and check membership in memory. The previous shape issued one
        // `SELECT name FROM _migrations WHERE name = ?` round-trip per
        // migration (17 of them at time of writing), which dominated the
        // cold-open cost of `Store::new` on warm caches. One SELECT is
        // O(N) network IO instead of O(N²); the in-memory check is free.
        let applied_rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM _migrations")
            .fetch_all(pool)
            .await
            .map_err(|e| MemoryError::sqlite("failed to load applied migrations", e))?;
        let applied: std::collections::HashSet<String> =
            applied_rows.into_iter().map(|(name,)| name).collect();

        for (name, sql) in migrations {
            if applied.contains(*name) {
                continue;
            }

            sqlx::raw_sql(sql)
                .execute(pool)
                .await
                .map_err(|e| MemoryError::sqlite(format!("migration {name} failed"), e))?;

            sqlx::query("INSERT INTO _migrations (name) VALUES (?)")
                .bind(name)
                .execute(pool)
                .await
                .map_err(|e| {
                    MemoryError::sqlite(format!("failed to record migration {name}"), e)
                })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
