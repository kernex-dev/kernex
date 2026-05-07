//! Reward-based learning: raw outcomes (working memory) and distilled lessons (long-term memory).
//!
//! All functions accept a `project` parameter for project-scoped isolation.
//! Empty string `""` = general (no project).

use super::Store;
use crate::error::MemoryError;

impl Store {
    /// Store a raw outcome from a REWARD marker.
    pub async fn store_outcome(
        &self,
        sender_id: &str,
        domain: &str,
        score: i32,
        lesson: &str,
        source: &str,
        project: &str,
    ) -> Result<(), MemoryError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO outcomes (id, sender_id, domain, score, lesson, source, project) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(sender_id)
        .bind(domain)
        .bind(score)
        .bind(lesson)
        .bind(source)
        .bind(project)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("store outcome", e))?;
        Ok(())
    }

    /// Get recent outcomes for a sender.
    ///
    /// When `project` is Some, returns only outcomes for that project.
    /// When `project` is None, returns all outcomes.
    pub async fn get_recent_outcomes(
        &self,
        sender_id: &str,
        limit: i64,
        project: Option<&str>,
    ) -> Result<Vec<(i32, String, String, String)>, MemoryError> {
        let rows: Vec<(i32, String, String, String)> = match project {
            Some(p) => {
                sqlx::query_as(
                    "SELECT score, domain, lesson, timestamp FROM outcomes \
                     WHERE sender_id = ? AND project = ? ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(sender_id)
                .bind(p)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as(
                    "SELECT score, domain, lesson, timestamp FROM outcomes \
                     WHERE sender_id = ? ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(sender_id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| MemoryError::sqlite("get recent outcomes", e))?;
        Ok(rows)
    }

    /// Get recent outcomes across all users.
    pub async fn get_all_recent_outcomes(
        &self,
        hours: i64,
        limit: i64,
        project: Option<&str>,
    ) -> Result<Vec<(i32, String, String, String)>, MemoryError> {
        let rows: Vec<(i32, String, String, String)> = match project {
            Some(p) => {
                sqlx::query_as(
                    "SELECT score, domain, lesson, timestamp FROM outcomes \
                     WHERE datetime(timestamp) >= datetime('now', ? || ' hours') \
                     AND project = ? \
                     ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(-hours)
                .bind(p)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as(
                    "SELECT score, domain, lesson, timestamp FROM outcomes \
                     WHERE datetime(timestamp) >= datetime('now', ? || ' hours') \
                     ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(-hours)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| MemoryError::sqlite("get all recent outcomes", e))?;
        Ok(rows)
    }

    /// Store a distilled lesson with content-based deduplication.
    ///
    /// Multiple lessons can exist per (sender_id, domain, project). If the exact
    /// same rule text already exists, its `occurrences` counter is bumped instead
    /// of creating a duplicate. After insertion, a cap of 10 lessons per
    /// (sender_id, domain, project) is enforced — oldest are pruned.
    pub async fn store_lesson(
        &self,
        sender_id: &str,
        domain: &str,
        rule: &str,
        project: &str,
    ) -> Result<(), MemoryError> {
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM lessons \
             WHERE sender_id = ? AND domain = ? AND project = ? AND rule = ?",
        )
        .bind(sender_id)
        .bind(domain)
        .bind(project)
        .bind(rule)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("store lesson check", e))?;

        if let Some((id,)) = existing {
            sqlx::query(
                "UPDATE lessons SET occurrences = occurrences + 1, \
                 updated_at = datetime('now') WHERE id = ?",
            )
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("store lesson reinforce", e))?;
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO lessons (id, sender_id, domain, rule, project) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(sender_id)
            .bind(domain)
            .bind(rule)
            .bind(project)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("store lesson insert", e))?;

            // Cap enforcement: keep at most 10 per (sender, domain, project).
            sqlx::query(
                "DELETE FROM lessons WHERE id IN ( \
                     SELECT id FROM lessons \
                     WHERE sender_id = ? AND domain = ? AND project = ? \
                     ORDER BY updated_at DESC, rowid DESC LIMIT -1 OFFSET 10 \
                 )",
            )
            .bind(sender_id)
            .bind(domain)
            .bind(project)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("store lesson cap", e))?;
        }

        Ok(())
    }

    /// Get lessons for a sender.
    ///
    /// When `project` is Some, returns project-specific lessons first, then general.
    /// When `project` is None, returns general lessons only (project = '').
    pub async fn get_lessons(
        &self,
        sender_id: &str,
        project: Option<&str>,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        let rows: Vec<(String, String, String)> = match project {
            Some(p) => {
                sqlx::query_as(
                    "SELECT domain, rule, project FROM lessons \
                     WHERE sender_id = ? AND (project = ? OR project = '') \
                     ORDER BY CASE WHEN project = ? THEN 0 ELSE 1 END, updated_at DESC \
                     LIMIT 50",
                )
                .bind(sender_id)
                .bind(p)
                .bind(p)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as(
                    "SELECT domain, rule, project FROM lessons \
                     WHERE sender_id = ? AND project = '' ORDER BY updated_at DESC \
                     LIMIT 50",
                )
                .bind(sender_id)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| MemoryError::sqlite("get lessons", e))?;
        Ok(rows)
    }

    /// Get all lessons across all users.
    pub async fn get_all_lessons(
        &self,
        project: Option<&str>,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        let rows: Vec<(String, String, String)> =
            match project {
                Some(p) => {
                    sqlx::query_as(
                        "SELECT domain, rule, project FROM lessons \
                     WHERE project = ? OR project = '' \
                     ORDER BY CASE WHEN project = ? THEN 0 ELSE 1 END, updated_at DESC \
                     LIMIT 50",
                    )
                    .bind(p)
                    .bind(p)
                    .fetch_all(&self.pool)
                    .await
                }
                None => sqlx::query_as(
                    "SELECT domain, rule, project FROM lessons ORDER BY updated_at DESC LIMIT 50",
                )
                .fetch_all(&self.pool)
                .await,
            }
            .map_err(|e| MemoryError::sqlite("get all lessons", e))?;
        Ok(rows)
    }
}
