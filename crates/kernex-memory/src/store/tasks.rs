//! Scheduled task CRUD, deduplication, and retry logic.

use super::Store;
use kernex_core::error::KernexError;
use uuid::Uuid;

/// A scheduled task that is due for delivery.
pub struct DueTask {
    pub id: String,
    pub channel: String,
    pub sender_id: String,
    pub reply_target: String,
    pub description: String,
    pub repeat: Option<String>,
    pub task_type: String,
    pub project: String,
}

impl Store {
    /// Create a scheduled task. Deduplicates on two levels:
    /// 1. Exact match: same sender + description + normalized due_at.
    /// 2. Fuzzy match: same sender + similar description + due_at within 30 min.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        channel: &str,
        sender_id: &str,
        reply_target: &str,
        description: &str,
        due_at: &str,
        repeat: Option<&str>,
        task_type: &str,
        project: &str,
    ) -> Result<String, KernexError> {
        let normalized_due = normalize_due_at(due_at);

        // Level 1: exact dedup on (sender, description, normalized due_at).
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM scheduled_tasks \
             WHERE sender_id = ? AND description = ? AND due_at = ? AND status = 'pending' \
             LIMIT 1",
        )
        .bind(sender_id)
        .bind(description)
        .bind(&normalized_due)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("dedup check failed: {e}")))?;

        if let Some((id,)) = existing {
            tracing::info!("scheduled task dedup: reusing existing {id}");
            return Ok(id);
        }

        // Level 2: fuzzy dedup — same sender, similar description, due_at within 30 min.
        let nearby: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, description, due_at FROM scheduled_tasks \
             WHERE sender_id = ? AND status = 'pending' \
             AND abs(strftime('%s', ?) - strftime('%s', due_at)) <= 1800",
        )
        .bind(sender_id)
        .bind(&normalized_due)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("fuzzy dedup check failed: {e}")))?;

        for (existing_id, existing_desc, _) in &nearby {
            if descriptions_are_similar(description, existing_desc) {
                tracing::info!(
                    "scheduled task fuzzy dedup: reusing {existing_id} (similar to new)"
                );
                return Ok(existing_id.clone());
            }
        }

        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO scheduled_tasks (id, channel, sender_id, reply_target, description, due_at, repeat, task_type, project) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(channel)
        .bind(sender_id)
        .bind(reply_target)
        .bind(description)
        .bind(&normalized_due)
        .bind(repeat)
        .bind(task_type)
        .bind(project)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("create task failed: {e}")))?;

        Ok(id)
    }

    /// Get tasks that are due for delivery.
    #[allow(clippy::type_complexity)]
    pub async fn get_due_tasks(&self) -> Result<Vec<DueTask>, KernexError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        )> = sqlx::query_as(
            "SELECT id, channel, sender_id, reply_target, description, repeat, task_type, project \
                 FROM scheduled_tasks \
                 WHERE status = 'pending' AND datetime(due_at) <= datetime('now')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get due tasks failed: {e}")))?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    channel,
                    sender_id,
                    reply_target,
                    description,
                    repeat,
                    task_type,
                    project,
                )| {
                    DueTask {
                        id,
                        channel,
                        sender_id,
                        reply_target,
                        description,
                        repeat,
                        task_type,
                        project,
                    }
                },
            )
            .collect())
    }

    /// Complete a task: one-shot tasks become 'delivered', recurring tasks advance due_at.
    pub async fn complete_task(&self, id: &str, repeat: Option<&str>) -> Result<(), KernexError> {
        match repeat {
            None | Some("once") => {
                sqlx::query(
                    "UPDATE scheduled_tasks SET status = 'delivered', delivered_at = datetime('now') WHERE id = ?",
                )
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("complete task failed: {e}")))?;
            }
            Some(interval) => {
                let offset = match interval {
                    "daily" | "weekdays" => "+1 day",
                    "weekly" => "+7 days",
                    "monthly" => "+1 month",
                    _ => "+1 day",
                };

                sqlx::query("UPDATE scheduled_tasks SET due_at = datetime(due_at, ?) WHERE id = ?")
                    .bind(offset)
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| KernexError::Store(format!("advance task failed: {e}")))?;

                if interval == "weekdays" {
                    sqlx::query(
                        "UPDATE scheduled_tasks SET due_at = datetime(due_at, '+2 days') \
                         WHERE id = ? AND CAST(strftime('%w', due_at) AS INTEGER) = 6",
                    )
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| KernexError::Store(format!("weekday skip sat failed: {e}")))?;

                    sqlx::query(
                        "UPDATE scheduled_tasks SET due_at = datetime(due_at, '+1 day') \
                         WHERE id = ? AND CAST(strftime('%w', due_at) AS INTEGER) = 0",
                    )
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| KernexError::Store(format!("weekday skip sun failed: {e}")))?;
                }
            }
        }
        Ok(())
    }

    /// Fail an action task: increment retry count and either reschedule or permanently fail.
    ///
    /// Returns `true` if the task will be retried, `false` if permanently failed.
    pub async fn fail_task(
        &self,
        id: &str,
        error: &str,
        max_retries: u32,
    ) -> Result<bool, KernexError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT retry_count FROM scheduled_tasks WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| KernexError::Store(format!("fail_task fetch failed: {e}")))?;

        let current_count = row.map(|r| r.0).unwrap_or(0) as u32;
        let new_count = current_count + 1;

        if new_count < max_retries {
            sqlx::query(
                "UPDATE scheduled_tasks \
                 SET retry_count = ?, last_error = ?, \
                     due_at = datetime('now', '+2 minutes') \
                 WHERE id = ?",
            )
            .bind(new_count as i64)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("fail_task retry update failed: {e}")))?;
            Ok(true)
        } else {
            sqlx::query(
                "UPDATE scheduled_tasks \
                 SET status = 'failed', retry_count = ?, last_error = ? \
                 WHERE id = ?",
            )
            .bind(new_count as i64)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("fail_task final update failed: {e}")))?;
            Ok(false)
        }
    }

    /// Get pending tasks for a sender.
    pub async fn get_tasks_for_sender(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String, Option<String>, String, String)>, KernexError> {
        let rows: Vec<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT id, description, due_at, repeat, task_type, project \
             FROM scheduled_tasks \
             WHERE sender_id = ? AND status = 'pending' \
             ORDER BY due_at ASC",
        )
        .bind(sender_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get tasks failed: {e}")))?;

        Ok(rows)
    }

    /// Cancel a task by ID prefix (must match sender).
    pub async fn cancel_task(&self, id_prefix: &str, sender_id: &str) -> Result<bool, KernexError> {
        let prefix = format!("{id_prefix}%");

        let result = sqlx::query(
            "UPDATE scheduled_tasks SET status = 'cancelled' \
             WHERE id LIKE ? AND sender_id = ? AND status = 'pending'",
        )
        .bind(&prefix)
        .bind(sender_id)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("cancel task failed: {e}")))?;

        if result.rows_affected() > 0 {
            return Ok(true);
        }

        let already: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scheduled_tasks \
             WHERE id LIKE ? AND sender_id = ? AND status = 'cancelled'",
        )
        .bind(&prefix)
        .bind(sender_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("cancel task check failed: {e}")))?;

        Ok(already.0 > 0)
    }

    /// Update fields of a pending task by ID prefix (must match sender).
    pub async fn update_task(
        &self,
        id_prefix: &str,
        sender_id: &str,
        description: Option<&str>,
        due_at: Option<&str>,
        repeat: Option<&str>,
    ) -> Result<bool, KernexError> {
        let mut sets = Vec::new();
        let mut values: Vec<String> = Vec::new();

        if let Some(d) = description {
            sets.push("description = ?");
            values.push(d.to_string());
        }
        if let Some(d) = due_at {
            sets.push("due_at = ?");
            values.push(d.to_string());
        }
        if let Some(r) = repeat {
            sets.push("repeat = ?");
            values.push(r.to_string());
        }

        if sets.is_empty() {
            return Ok(false);
        }

        let sql = format!(
            "UPDATE scheduled_tasks SET {} WHERE id LIKE ? AND sender_id = ? AND status = 'pending'",
            sets.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for v in &values {
            query = query.bind(v);
        }
        query = query.bind(format!("{id_prefix}%"));
        query = query.bind(sender_id);

        let result = query
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("update task failed: {e}")))?;

        Ok(result.rows_affected() > 0)
    }

    /// Defer a pending task to a new due_at time (by exact ID).
    pub async fn defer_task(&self, id: &str, new_due_at: &str) -> Result<(), KernexError> {
        sqlx::query("UPDATE scheduled_tasks SET due_at = ? WHERE id = ? AND status = 'pending'")
            .bind(new_due_at)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("defer task failed: {e}")))?;
        Ok(())
    }
}

/// Normalize a datetime string to a consistent format for dedup comparison.
pub(super) fn normalize_due_at(due_at: &str) -> String {
    let s = due_at.trim_end_matches('Z');
    s.replacen('T', " ", 1)
}

/// Check if two task descriptions are semantically similar via word overlap.
pub(super) fn descriptions_are_similar(a: &str, b: &str) -> bool {
    let words_a = significant_words(a);
    let words_b = significant_words(b);

    if words_a.len() < 3 || words_b.len() < 3 {
        return false;
    }

    let (smaller, larger) = if words_a.len() <= words_b.len() {
        (&words_a, &words_b)
    } else {
        (&words_b, &words_a)
    };

    let overlap = smaller.iter().filter(|w| larger.contains(w)).count();
    let threshold = smaller.len().div_ceil(2);
    overlap >= threshold
}

fn significant_words(text: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "and", "for", "that", "this", "with", "from", "are", "was", "were", "been", "have",
        "has", "had", "will", "would", "could", "should", "may", "might", "can", "about", "into",
        "over", "after", "before", "between", "under", "again", "then", "once", "daily", "weekly",
        "monthly", "cada", "diario", "escribir", "enviar", "usar", "nunca", "siempre", "cada",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}
