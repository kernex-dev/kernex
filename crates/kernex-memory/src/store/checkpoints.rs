//! Phase checkpoint storage for resumable pipeline runs.
//!
//! Each checkpoint records the status and output of one pipeline phase
//! within a run. A `run_id` (UUID) groups all phases belonging to the
//! same pipeline execution, allowing a failed run to be resumed from
//! the last completed phase.
//!
//! # Lifecycle
//!
//! ```text
//! upsert_phase_checkpoint(run_id, phase, "pending",     None,   None)
//! upsert_phase_checkpoint(run_id, phase, "in_progress", None,   None)
//! upsert_phase_checkpoint(run_id, phase, "completed",   output, None)
//! // or
//! upsert_phase_checkpoint(run_id, phase, "failed",      None,   Some(error))
//! ```
//!
//! To resume a run, call `get_run_checkpoints` and skip phases whose
//! `status` is `"completed"`.

use super::Store;
use kernex_core::error::KernexError;

type CheckpointRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    String,
);

/// A recorded snapshot of one pipeline phase within a run.
#[derive(Debug, Clone)]
pub struct PhaseCheckpoint {
    /// Unique row identifier.
    pub id: String,
    /// UUID identifying the enclosing pipeline run.
    pub run_id: String,
    /// Name of the topology (e.g. `"my-pipeline"`).
    pub topology_name: String,
    /// Name of the phase within the topology.
    pub phase_name: String,
    /// Agent / sender identifier that owns this run.
    pub sender_id: String,
    /// Project scope. Empty string = no project.
    pub project: String,
    /// One of `"pending"`, `"in_progress"`, `"completed"`, `"failed"`.
    pub status: String,
    /// Phase output text. `None` until the phase completes successfully.
    pub output: Option<String>,
    /// Error detail. Set only when `status` is `"failed"`.
    pub error_message: Option<String>,
    /// How many attempts have been made for this phase (0-indexed).
    pub attempt: i64,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 last-update timestamp.
    pub updated_at: String,
}

impl Store {
    /// Create or update a checkpoint for one phase within a run.
    ///
    /// Uses `INSERT OR REPLACE` keyed on `(run_id, phase_name)`, so calling
    /// this multiple times as the phase progresses is safe and idempotent.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_phase_checkpoint(
        &self,
        run_id: &str,
        topology_name: &str,
        phase_name: &str,
        sender_id: &str,
        project: &str,
        status: &str,
        output: Option<&str>,
        error_message: Option<&str>,
        attempt: i64,
    ) -> Result<(), KernexError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO phase_checkpoints \
             (id, run_id, topology_name, phase_name, sender_id, project, \
              status, output, error_message, attempt) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (run_id, phase_name) DO UPDATE SET \
               topology_name = excluded.topology_name, \
               sender_id     = excluded.sender_id, \
               project       = excluded.project, \
               status        = excluded.status, \
               output        = excluded.output, \
               error_message = excluded.error_message, \
               attempt       = excluded.attempt, \
               updated_at    = datetime('now')",
        )
        .bind(&id)
        .bind(run_id)
        .bind(topology_name)
        .bind(phase_name)
        .bind(sender_id)
        .bind(project)
        .bind(status)
        .bind(output)
        .bind(error_message)
        .bind(attempt)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("upsert phase checkpoint: {e}")))?;
        Ok(())
    }

    /// Fetch the checkpoint for a specific phase within a run.
    pub async fn get_phase_checkpoint(
        &self,
        run_id: &str,
        phase_name: &str,
    ) -> Result<Option<PhaseCheckpoint>, KernexError> {
        let row: Option<CheckpointRow> = sqlx::query_as(
            "SELECT id, run_id, topology_name, phase_name, sender_id, project, \
                    status, output, error_message, attempt, created_at, updated_at \
             FROM phase_checkpoints WHERE run_id = ? AND phase_name = ?",
        )
        .bind(run_id)
        .bind(phase_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get phase checkpoint: {e}")))?;

        Ok(row.map(
            |(
                id,
                run_id,
                topology_name,
                phase_name,
                sender_id,
                project,
                status,
                output,
                error_message,
                attempt,
                created_at,
                updated_at,
            )| PhaseCheckpoint {
                id,
                run_id,
                topology_name,
                phase_name,
                sender_id,
                project,
                status,
                output,
                error_message,
                attempt,
                created_at,
                updated_at,
            },
        ))
    }

    /// Fetch all phase checkpoints for a run, ordered by creation time.
    ///
    /// Use this to inspect which phases have already completed when resuming
    /// a failed run.
    pub async fn get_run_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<PhaseCheckpoint>, KernexError> {
        let rows: Vec<CheckpointRow> = sqlx::query_as(
            "SELECT id, run_id, topology_name, phase_name, sender_id, project, \
                    status, output, error_message, attempt, created_at, updated_at \
             FROM phase_checkpoints WHERE run_id = ? ORDER BY created_at ASC",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("get run checkpoints: {e}")))?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    run_id,
                    topology_name,
                    phase_name,
                    sender_id,
                    project,
                    status,
                    output,
                    error_message,
                    attempt,
                    created_at,
                    updated_at,
                )| PhaseCheckpoint {
                    id,
                    run_id,
                    topology_name,
                    phase_name,
                    sender_id,
                    project,
                    status,
                    output,
                    error_message,
                    attempt,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// Delete all checkpoints for a run.
    ///
    /// Call this after a pipeline run completes successfully to reclaim space,
    /// or before re-running a pipeline from scratch.
    pub async fn clear_run_checkpoints(&self, run_id: &str) -> Result<(), KernexError> {
        sqlx::query("DELETE FROM phase_checkpoints WHERE run_id = ?")
            .bind(run_id)
            .execute(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("clear run checkpoints: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernex_core::config::MemoryConfig;

    async fn test_store() -> Store {
        let tmp = std::env::temp_dir().join(format!(
            "__kernex_checkpoints_test_{}__{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let config = MemoryConfig {
            db_path: tmp.to_str().unwrap().to_string(),
            ..Default::default()
        };
        Store::new(&config).await.unwrap()
    }

    #[tokio::test]
    async fn test_upsert_and_get_checkpoint() {
        let store = test_store().await;
        let run_id = uuid::Uuid::new_v4().to_string();

        store
            .upsert_phase_checkpoint(
                &run_id,
                "my-pipeline",
                "phase-1",
                "user-1",
                "",
                "completed",
                Some("phase output"),
                None,
                0,
            )
            .await
            .unwrap();

        let cp = store
            .get_phase_checkpoint(&run_id, "phase-1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(cp.run_id, run_id);
        assert_eq!(cp.topology_name, "my-pipeline");
        assert_eq!(cp.phase_name, "phase-1");
        assert_eq!(cp.status, "completed");
        assert_eq!(cp.output.as_deref(), Some("phase output"));
        assert!(cp.error_message.is_none());
    }

    #[tokio::test]
    async fn test_upsert_updates_existing() {
        let store = test_store().await;
        let run_id = uuid::Uuid::new_v4().to_string();

        store
            .upsert_phase_checkpoint(
                &run_id,
                "topo",
                "phase-a",
                "user-1",
                "",
                "in_progress",
                None,
                None,
                0,
            )
            .await
            .unwrap();

        store
            .upsert_phase_checkpoint(
                &run_id,
                "topo",
                "phase-a",
                "user-1",
                "",
                "completed",
                Some("done"),
                None,
                0,
            )
            .await
            .unwrap();

        let cp = store
            .get_phase_checkpoint(&run_id, "phase-a")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(cp.status, "completed");
        assert_eq!(cp.output.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn test_get_run_checkpoints_ordered() {
        let store = test_store().await;
        let run_id = uuid::Uuid::new_v4().to_string();

        for phase in &["phase-1", "phase-2", "phase-3"] {
            store
                .upsert_phase_checkpoint(
                    &run_id,
                    "topo",
                    phase,
                    "user-1",
                    "",
                    "completed",
                    None,
                    None,
                    0,
                )
                .await
                .unwrap();
        }

        let checkpoints = store.get_run_checkpoints(&run_id).await.unwrap();
        assert_eq!(checkpoints.len(), 3);
        assert_eq!(checkpoints[0].phase_name, "phase-1");
        assert_eq!(checkpoints[1].phase_name, "phase-2");
        assert_eq!(checkpoints[2].phase_name, "phase-3");
    }

    #[tokio::test]
    async fn test_clear_run_checkpoints() {
        let store = test_store().await;
        let run_id = uuid::Uuid::new_v4().to_string();

        store
            .upsert_phase_checkpoint(
                &run_id,
                "topo",
                "phase-1",
                "user-1",
                "",
                "completed",
                None,
                None,
                0,
            )
            .await
            .unwrap();

        store.clear_run_checkpoints(&run_id).await.unwrap();

        let checkpoints = store.get_run_checkpoints(&run_id).await.unwrap();
        assert!(checkpoints.is_empty());
    }

    #[tokio::test]
    async fn test_failed_checkpoint_stores_error() {
        let store = test_store().await;
        let run_id = uuid::Uuid::new_v4().to_string();

        store
            .upsert_phase_checkpoint(
                &run_id,
                "topo",
                "phase-1",
                "user-1",
                "proj-a",
                "failed",
                None,
                Some("provider timeout"),
                1,
            )
            .await
            .unwrap();

        let cp = store
            .get_phase_checkpoint(&run_id, "phase-1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(cp.status, "failed");
        assert_eq!(cp.error_message.as_deref(), Some("provider timeout"));
        assert_eq!(cp.attempt, 1);
        assert_eq!(cp.project, "proj-a");
    }
}
