//! Scheduled task CRUD, deduplication, and retry logic.

use super::Store;
use crate::error::MemoryError;
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

/// One recorded execution of a scheduled task.
#[derive(Debug, Clone)]
pub struct TaskRunRecord {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub finished_at: String,
    /// `"completed"` or `"failed"` (enforced by a DB CHECK).
    pub status: String,
    /// Response text on success.
    pub result: Option<String>,
    /// Error text on failure.
    pub error: Option<String>,
    /// Billed tokens for the run, when the provider reported a count.
    pub tokens_used: Option<i64>,
}

/// A claim older than this is considered abandoned (the claimer died
/// mid-run) and becomes reclaimable by the next poller.
const CLAIM_STALE_MINUTES: u32 = 10;

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
    ) -> Result<String, MemoryError> {
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
        .map_err(|e| MemoryError::sqlite("dedup check failed", e))?;

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
        .map_err(|e| MemoryError::sqlite("fuzzy dedup check failed", e))?;

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
        .map_err(|e| MemoryError::sqlite("create task failed", e))?;

        Ok(id)
    }

    /// Get tasks that are due for delivery.
    #[allow(clippy::type_complexity)]
    pub async fn get_due_tasks(&self) -> Result<Vec<DueTask>, MemoryError> {
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
        .map_err(|e| MemoryError::sqlite("get due tasks failed", e))?;

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

    /// Atomically claim every task that is due: status flips
    /// 'pending' -> 'claimed' and the claimed rows are returned in the same
    /// statement, so when several pollers (an HTTP server, an open REPL, a
    /// one-shot drain) share a store, each due task is handed to exactly one
    /// of them. Claims left behind by a dead claimer become reclaimable
    /// after [`CLAIM_STALE_MINUTES`].
    ///
    /// The claim is released by [`Store::complete_task`] (recurring tasks
    /// return to 'pending' at the next due time) or [`Store::fail_task`]
    /// (retries return to 'pending').
    pub async fn claim_due_tasks(&self) -> Result<Vec<DueTask>, MemoryError> {
        #[allow(clippy::type_complexity)]
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
            "UPDATE scheduled_tasks \
             SET status = 'claimed', claimed_at = datetime('now') \
             WHERE (status = 'pending' AND datetime(due_at) <= datetime('now')) \
                OR (status = 'claimed' AND datetime(claimed_at) <= datetime('now', ?)) \
             RETURNING id, channel, sender_id, reply_target, description, repeat, task_type, project",
        )
        .bind(format!("-{CLAIM_STALE_MINUTES} minutes"))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("claim due tasks failed", e))?;

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

    /// Record one execution of a scheduled task. `status` must be
    /// `"completed"` or `"failed"` (DB CHECK); `finished_at` is stamped
    /// server-side. Returns the new run id.
    pub async fn record_task_run(
        &self,
        task_id: &str,
        started_at: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
        tokens_used: Option<u64>,
    ) -> Result<String, MemoryError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO task_runs (id, task_id, started_at, status, result, error, tokens_used) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(task_id)
        .bind(started_at)
        .bind(status)
        .bind(result)
        .bind(error)
        .bind(tokens_used.map(|t| t as i64))
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("record task run failed", e))?;
        Ok(id)
    }

    /// Recorded runs for tasks whose id starts with `task_id_prefix`,
    /// newest first, capped at `limit`.
    pub async fn list_task_runs(
        &self,
        task_id_prefix: &str,
        limit: u32,
    ) -> Result<Vec<TaskRunRecord>, MemoryError> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
        )> = sqlx::query_as(
            "SELECT id, task_id, started_at, finished_at, status, result, error, tokens_used \
             FROM task_runs WHERE task_id LIKE ? \
             ORDER BY started_at DESC LIMIT ?",
        )
        .bind(format!("{task_id_prefix}%"))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("list task runs failed", e))?;

        Ok(rows
            .into_iter()
            .map(
                |(id, task_id, started_at, finished_at, status, result, error, tokens_used)| {
                    TaskRunRecord {
                        id,
                        task_id,
                        started_at,
                        finished_at,
                        status,
                        result,
                        error,
                        tokens_used,
                    }
                },
            )
            .collect())
    }

    /// Complete a task: one-shot tasks become 'delivered', recurring tasks advance due_at.
    pub async fn complete_task(&self, id: &str, repeat: Option<&str>) -> Result<(), MemoryError> {
        match repeat {
            None | Some("once") => {
                sqlx::query(
                    "UPDATE scheduled_tasks SET status = 'delivered', delivered_at = datetime('now') WHERE id = ?",
                )
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| MemoryError::sqlite("complete task failed", e))?;
            }
            Some(interval) => {
                let offset = match interval {
                    "daily" | "weekdays" => "+1 day",
                    "weekly" => "+7 days",
                    "monthly" => "+1 month",
                    _ => "+1 day",
                };

                // Advancing a recurring task also releases any claim so the
                // next occurrence is visible to pollers again.
                sqlx::query(
                    "UPDATE scheduled_tasks \
                     SET due_at = datetime(due_at, ?), status = 'pending', claimed_at = NULL \
                     WHERE id = ?",
                )
                .bind(offset)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| MemoryError::sqlite("advance task failed", e))?;

                if interval == "weekdays" {
                    sqlx::query(
                        "UPDATE scheduled_tasks SET due_at = datetime(due_at, '+2 days') \
                         WHERE id = ? AND CAST(strftime('%w', due_at) AS INTEGER) = 6",
                    )
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| MemoryError::sqlite("weekday skip sat failed", e))?;

                    sqlx::query(
                        "UPDATE scheduled_tasks SET due_at = datetime(due_at, '+1 day') \
                         WHERE id = ? AND CAST(strftime('%w', due_at) AS INTEGER) = 0",
                    )
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| MemoryError::sqlite("weekday skip sun failed", e))?;
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
    ) -> Result<bool, MemoryError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT retry_count FROM scheduled_tasks WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MemoryError::sqlite("fail_task fetch failed", e))?;

        let current_count = row.map(|r| r.0).unwrap_or(0) as u32;
        let new_count = current_count + 1;

        if new_count < max_retries {
            // The retry also releases any claim so the rescheduled attempt
            // is visible to pollers again.
            sqlx::query(
                "UPDATE scheduled_tasks \
                 SET retry_count = ?, last_error = ?, \
                     due_at = datetime('now', '+2 minutes'), \
                     status = 'pending', claimed_at = NULL \
                 WHERE id = ?",
            )
            .bind(new_count as i64)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("fail_task retry update failed", e))?;
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
            .map_err(|e| MemoryError::sqlite("fail_task final update failed", e))?;
            Ok(false)
        }
    }

    /// Get pending tasks for a sender.
    pub async fn get_tasks_for_sender(
        &self,
        sender_id: &str,
    ) -> Result<Vec<(String, String, String, Option<String>, String, String)>, MemoryError> {
        let rows: Vec<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT id, description, due_at, repeat, task_type, project \
             FROM scheduled_tasks \
             WHERE sender_id = ? AND status = 'pending' \
             ORDER BY due_at ASC",
        )
        .bind(sender_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("get tasks failed", e))?;

        Ok(rows)
    }

    /// Cancel a task by ID prefix (must match sender).
    pub async fn cancel_task(&self, id_prefix: &str, sender_id: &str) -> Result<bool, MemoryError> {
        let prefix = format!("{id_prefix}%");

        let result = sqlx::query(
            "UPDATE scheduled_tasks SET status = 'cancelled' \
             WHERE id LIKE ? AND sender_id = ? AND status = 'pending'",
        )
        .bind(&prefix)
        .bind(sender_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("cancel task failed", e))?;

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
        .map_err(|e| MemoryError::sqlite("cancel task check failed", e))?;

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
    ) -> Result<bool, MemoryError> {
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
            .map_err(|e| MemoryError::sqlite("update task failed", e))?;

        Ok(result.rows_affected() > 0)
    }

    /// Defer a pending task to a new due_at time (by exact ID).
    pub async fn defer_task(&self, id: &str, new_due_at: &str) -> Result<(), MemoryError> {
        sqlx::query("UPDATE scheduled_tasks SET due_at = ? WHERE id = ? AND status = 'pending'")
            .bind(new_due_at)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::sqlite("defer task failed", e))?;
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
