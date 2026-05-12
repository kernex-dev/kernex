//! Typed observation log.
//!
//! Read / write paths over the `observations` table introduced by
//! migration 018. Mirrors the soft-delete discipline established in
//! migration 017 for facts: the trait surface in
//! [`crate::memory_store::MemoryStore`] exposes only the safe paths
//! (`save`, `get`, `search`, `soft_delete`, `list_soft_deleted`);
//! hard-delete is intentionally absent and reserved for emergency
//! cleanup via the inherent surface.

use std::time::SystemTime;

use super::Store;
use crate::error::MemoryError;
use crate::observation::{Observation, ObservationType, SaveEntry};
use crate::types::{format_sqlite_timestamp, parse_sqlite_timestamp};
use uuid::Uuid;

/// Tuple shape pulled by all SELECTs. Order matches the column order
/// declared in migration `018_observations.sql`.
type ObservationTuple = (
    String,         // id
    String,         // sender_id
    String,         // type (as DB string)
    String,         // title
    Option<String>, // what
    Option<String>, // why
    Option<String>, // where_field
    Option<String>, // learned
    String,         // created_at (SQLite timestamp string)
    String,         // updated_at
);

fn tuple_to_observation(row: ObservationTuple) -> Result<Observation, MemoryError> {
    let (id, sender_id, kind_str, title, what, why, where_field, learned, created_at, updated_at) =
        row;
    let kind = ObservationType::from_db_str(&kind_str).ok_or_else(|| {
        MemoryError::logic(format!(
            "observation row {id} carries unknown type `{kind_str}` not in the current ObservationType enum"
        ))
    })?;
    Ok(Observation {
        id,
        sender_id,
        kind,
        title,
        what,
        why,
        where_field,
        learned,
        created_at: parse_sqlite_timestamp(&created_at)?,
        updated_at: parse_sqlite_timestamp(&updated_at)?,
    })
}

impl Store {
    /// Persist a typed observation. Generates a fresh UUIDv4 id and
    /// sets `created_at == updated_at == now`. Returns the saved row.
    ///
    /// The DB enforces two CHECK constraints that surface as
    /// `MemoryError::Sqlite { source: sqlx::Error::Database(..), .. }`:
    /// - `length(title) > 0`
    /// - `type IN (<seven enum strings>)` (only reachable if the caller
    ///   bypasses `ObservationType` and writes a raw string)
    pub async fn save_observation(&self, entry: SaveEntry) -> Result<Observation, MemoryError> {
        let id = Uuid::new_v4().to_string();
        let now = SystemTime::now();
        let now_str = format_sqlite_timestamp(now);
        let kind_str = entry.kind.as_db_str();

        sqlx::query(
            "INSERT INTO observations \
                (id, sender_id, type, title, what, why, where_field, learned, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&entry.sender_id)
        .bind(kind_str)
        .bind(&entry.title)
        .bind(&entry.what)
        .bind(&entry.why)
        .bind(&entry.where_field)
        .bind(&entry.learned)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("insert observation failed", e))?;

        Ok(Observation {
            id,
            sender_id: entry.sender_id,
            kind: entry.kind,
            title: entry.title,
            what: entry.what,
            why: entry.why,
            where_field: entry.where_field,
            learned: entry.learned,
            created_at: now,
            updated_at: now,
        })
    }

    /// Fetch an active observation by id. Returns `None` when the id
    /// is missing OR the row is soft-deleted (CC-9 invariant).
    pub async fn get_observation_by_id(
        &self,
        id: &str,
    ) -> Result<Option<Observation>, MemoryError> {
        let row: Option<ObservationTuple> = sqlx::query_as(
            "SELECT id, sender_id, type, title, what, why, where_field, learned, \
                    created_at, updated_at \
             FROM observations \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("get_observation_by_id failed", e))?;

        match row {
            Some(tuple) => Ok(Some(tuple_to_observation(tuple)?)),
            None => Ok(None),
        }
    }

    /// FTS5 search across `title`, `what`, `why`, `where_field`,
    /// `learned`. Optional `since` filters by `created_at >=`; optional
    /// `kind` narrows to a single observation type. Soft-deleted rows
    /// never appear (the FTS5 triggers on `observations_au` /
    /// `observations_ad` keep the mirror in sync).
    ///
    /// Result order: FTS5 `rank` ascending (best match first), then
    /// `created_at` descending tiebreaker.
    pub async fn search_observations(
        &self,
        query: &str,
        sender_id: &str,
        limit: i64,
        since: Option<SystemTime>,
        kind: Option<ObservationType>,
    ) -> Result<Vec<Observation>, MemoryError> {
        if query.len() < 3 {
            return Ok(Vec::new());
        }

        // Double-quote the query to neutralize FTS5 operators (AND, OR,
        // NOT, NEAR, *, etc.) and escape internal double quotes by
        // doubling them, matching the pattern in messages.rs.
        let sanitized = format!("\"{}\"", query.replace('"', "\"\""));

        // Build the SQL dynamically to keep parameter binds clean.
        // sqlx caches prepared statements per (statement, parameter
        // shape); the 4 combinations of (since, kind) become 4 cache
        // entries, the same cardinality as messages.rs's 2 variants for
        // `since`.
        let mut sql = String::from(
            "SELECT o.id, o.sender_id, o.type, o.title, o.what, o.why, \
                    o.where_field, o.learned, o.created_at, o.updated_at \
             FROM observations_fts fts \
             JOIN observations o ON o.rowid = fts.rowid \
             WHERE observations_fts MATCH ? \
             AND o.sender_id = ? \
             AND o.deleted_at IS NULL",
        );
        if since.is_some() {
            sql.push_str(" AND o.created_at >= ?");
        }
        if kind.is_some() {
            sql.push_str(" AND o.type = ?");
        }
        sql.push_str(" ORDER BY rank, o.created_at DESC LIMIT ?");

        let mut q = sqlx::query_as::<_, ObservationTuple>(&sql)
            .bind(&sanitized)
            .bind(sender_id);
        if let Some(cutoff) = since {
            q = q.bind(format_sqlite_timestamp(cutoff));
        }
        if let Some(k) = kind {
            q = q.bind(k.as_db_str());
        }
        let rows: Vec<ObservationTuple> = q
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("search_observations failed", e))?;

        let mut out = Vec::with_capacity(rows.len());
        for tuple in rows {
            out.push(tuple_to_observation(tuple)?);
        }
        Ok(out)
    }

    /// Soft-delete an observation by setting `deleted_at` to "now".
    /// Returns `Ok(true)` if a row transitioned from active to deleted,
    /// `Ok(false)` if the row was already deleted, missing, or never
    /// existed (matches the `soft_delete_fact` contract).
    ///
    /// The `observations_au` trigger drops the row from
    /// `observations_fts` automatically; consumers do not need to
    /// touch the FTS mirror.
    pub async fn soft_delete_observation(&self, id: &str) -> Result<bool, MemoryError> {
        let now_str = format_sqlite_timestamp(SystemTime::now());
        let result = sqlx::query(
            "UPDATE observations SET deleted_at = ?, updated_at = ? \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(&now_str)
        .bind(&now_str)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("soft delete observation failed", e))?;

        Ok(result.rows_affected() > 0)
    }

    /// Read soft-deleted observations for a sender. Recovery helper;
    /// the trait exposes this so future tooling can offer an
    /// "undelete" surface without dropping back to the inherent
    /// `Store`. Order: most-recently-deleted first.
    pub async fn list_soft_deleted_observations(
        &self,
        sender_id: &str,
    ) -> Result<Vec<Observation>, MemoryError> {
        let rows: Vec<ObservationTuple> = sqlx::query_as(
            "SELECT id, sender_id, type, title, what, why, where_field, learned, \
                    created_at, updated_at \
             FROM observations \
             WHERE sender_id = ? AND deleted_at IS NOT NULL \
             ORDER BY deleted_at DESC",
        )
        .bind(sender_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("list_soft_deleted_observations failed", e))?;

        let mut out = Vec::with_capacity(rows.len());
        for tuple in rows {
            out.push(tuple_to_observation(tuple)?);
        }
        Ok(out)
    }

    /// Count of active (non-soft-deleted) observations for a sender.
    /// Used by the 4-tuple `get_memory_stats` extension; see
    /// [`Self::get_memory_stats`].
    pub(crate) async fn count_observations(&self, sender_id: &str) -> Result<i64, MemoryError> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM observations \
             WHERE sender_id = ? AND deleted_at IS NULL",
        )
        .bind(sender_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("count_observations failed", e))?;
        Ok(count)
    }
}
