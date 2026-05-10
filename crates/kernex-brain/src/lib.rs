#![doc = "Workspace-internal scaffold for the kernex memory brain. Trait surface only; implementations land in a follow-up change."]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use thiserror::Error;

/// Composite health score 0-100 for a project's memory store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthScore {
    pub project: String,
    pub score: u8,
}

/// Pairwise relation between two stored observations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConflictRelation {
    pub left_id: i64,
    pub right_id: i64,
    pub kind: String,
}

/// Forgetting-risk ranking entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecayRanking {
    pub observation_id: i64,
    pub risk: f32,
    pub last_accessed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BrainError {
    #[error("unsupported operation in this build")]
    Unsupported,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Trait surface for the kernex memory brain. Stub-only scaffold; method
/// bodies return [`BrainError::Unsupported`] until a follow-up change replaces
/// them.
#[async_trait::async_trait]
pub trait BrainStore: Send + Sync {
    /// Persist a new observation into the brain layer.
    async fn record(&self, project: &str, payload: &str) -> Result<i64, BrainError>;

    /// Retrieve observations matching a query string.
    async fn search(&self, project: &str, query: &str) -> Result<Vec<i64>, BrainError>;

    /// Composite health score for a project's brain layer.
    async fn health(&self, project: &str) -> Result<HealthScore, BrainError>;

    /// Forgetting-risk ranking for a project.
    async fn decay(
        &self,
        project: &str,
        horizon_days: u32,
    ) -> Result<Vec<DecayRanking>, BrainError>;
}
