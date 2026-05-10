#![doc = "Workspace-internal scaffold for the kernex memory brain. Trait surface only; implementations land in a follow-up change."]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use thiserror::Error;

/// Opaque identifier for a stored observation. Newtype so the underlying
/// representation can change without a breaking signature change on the
/// trait methods that accept or return it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ObservationId(pub i64);

/// Composite health score 0-100 for a project's memory store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct HealthScore {
    pub project: String,
    pub score: u8,
}

impl HealthScore {
    pub fn new(project: String, score: u8) -> Self {
        Self { project, score }
    }
}

/// Pairwise relation between two stored observations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ConflictRelation {
    pub left_id: ObservationId,
    pub right_id: ObservationId,
    pub kind: String,
}

impl ConflictRelation {
    pub fn new(left_id: ObservationId, right_id: ObservationId, kind: String) -> Self {
        Self {
            left_id,
            right_id,
            kind,
        }
    }
}

/// Forgetting-risk ranking entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct DecayRanking {
    pub observation_id: ObservationId,
    pub risk: f32,
    pub last_accessed_at: chrono::DateTime<chrono::Utc>,
}

impl DecayRanking {
    pub fn new(
        observation_id: ObservationId,
        risk: f32,
        last_accessed_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            observation_id,
            risk,
            last_accessed_at,
        }
    }
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
    async fn record(&self, project: &str, payload: &str) -> Result<ObservationId, BrainError>;

    /// Retrieve observations matching a query string.
    async fn search(&self, project: &str, query: &str) -> Result<Vec<ObservationId>, BrainError>;

    /// Composite health score for a project's brain layer.
    async fn health(&self, project: &str) -> Result<HealthScore, BrainError>;

    /// Forgetting-risk ranking for a project.
    async fn decay(
        &self,
        project: &str,
        horizon_days: u32,
    ) -> Result<Vec<DecayRanking>, BrainError>;
}
