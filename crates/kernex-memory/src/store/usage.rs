//! Token usage recording and cost tracking.

use super::Store;
use crate::error::MemoryError;
use kernex_core::pricing::pricing_for;

/// Per-dimension token breakdown reported by providers that distinguish
/// regular input, output, prompt-cache reads, and prompt-cache creations
/// (e.g. Anthropic). All fields are optional — providers that do not report
/// a breakdown should pass `UsageBreakdown::default()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UsageBreakdown {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

/// Aggregated token usage for a session or sender.
#[derive(Debug, Clone, Default)]
pub struct UsageSummary {
    /// Total tokens consumed across all recorded requests.
    pub total_tokens: i64,
    /// Estimated total cost in USD.
    pub total_cost_usd: f64,
    /// Number of API requests recorded.
    pub request_count: i64,
    /// Sum of input tokens across requests that reported a breakdown.
    pub total_input_tokens: i64,
    /// Sum of output tokens across requests that reported a breakdown.
    pub total_output_tokens: i64,
    /// Sum of prompt-cache reads across requests that reported a breakdown.
    pub total_cache_read_tokens: i64,
    /// Sum of prompt-cache creations across requests that reported a breakdown.
    pub total_cache_creation_tokens: i64,
}

impl Store {
    /// Record token usage for a completed API request, total tokens only.
    ///
    /// Thin wrapper over [`Store::record_usage_full`] for callers that do not
    /// have a per-dimension breakdown. Cost is estimated using known per-model
    /// pricing; unrecognized models record cost as 0.0.
    pub async fn record_usage(
        &self,
        sender_id: &str,
        session_id: &str,
        tokens: u64,
        model: &str,
    ) -> Result<(), MemoryError> {
        self.record_usage_full(
            sender_id,
            session_id,
            tokens,
            model,
            UsageBreakdown::default(),
        )
        .await
    }

    /// Record token usage with a per-dimension breakdown.
    ///
    /// `tokens` is the authoritative total used for cost estimation and
    /// summary aggregation. The breakdown columns are stored verbatim and
    /// surface in [`UsageSummary`] for cost telemetry (e.g. cache hit ratio).
    pub async fn record_usage_full(
        &self,
        sender_id: &str,
        session_id: &str,
        tokens: u64,
        model: &str,
        breakdown: UsageBreakdown,
    ) -> Result<(), MemoryError> {
        let cost = pricing_for(model)
            .map(|p| p.estimate_cost(tokens))
            .unwrap_or(0.0);

        sqlx::query(
            "INSERT INTO token_usage (
                 sender_id, session_id, model, tokens, cost_usd,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens
             )
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(sender_id)
        .bind(session_id)
        .bind(model)
        .bind(tokens as i64)
        .bind(cost)
        .bind(breakdown.input_tokens.map(|v| v as i64))
        .bind(breakdown.output_tokens.map(|v| v as i64))
        .bind(breakdown.cache_read_tokens.map(|v| v as i64))
        .bind(breakdown.cache_creation_tokens.map(|v| v as i64))
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("failed to record token usage", e))?;

        Ok(())
    }

    /// Get aggregated token usage for a session.
    pub async fn get_session_usage(&self, session_id: &str) -> Result<UsageSummary, MemoryError> {
        let row: Option<(i64, f64, i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT
                 COALESCE(SUM(tokens), 0),
                 COALESCE(SUM(cost_usd), 0.0),
                 COUNT(*),
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_read_tokens), 0),
                 COALESCE(SUM(cache_creation_tokens), 0)
             FROM token_usage WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("failed to query session usage", e))?;

        let (
            total_tokens,
            total_cost_usd,
            request_count,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_creation_tokens,
        ) = row.unwrap_or((0, 0.0, 0, 0, 0, 0, 0));

        Ok(UsageSummary {
            total_tokens,
            total_cost_usd,
            request_count,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_creation_tokens,
        })
    }

    /// Get aggregated token usage across all sessions in the store.
    ///
    /// Useful for project-wide cost reporting (e.g. the kx `/cost`
    /// command) when callers do not maintain a stable session id.
    pub async fn get_total_usage(&self) -> Result<UsageSummary, MemoryError> {
        let row: Option<(i64, f64, i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT
                 COALESCE(SUM(tokens), 0),
                 COALESCE(SUM(cost_usd), 0.0),
                 COUNT(*),
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_read_tokens), 0),
                 COALESCE(SUM(cache_creation_tokens), 0)
             FROM token_usage",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::sqlite("failed to query total usage", e))?;

        let (
            total_tokens,
            total_cost_usd,
            request_count,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_creation_tokens,
        ) = row.unwrap_or((0, 0.0, 0, 0, 0, 0, 0));

        Ok(UsageSummary {
            total_tokens,
            total_cost_usd,
            request_count,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_creation_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernex_core::config::MemoryConfig;

    async fn make_store() -> Store {
        let config = MemoryConfig {
            db_path: ":memory:".to_string(),
            ..Default::default()
        };
        Store::new(&config).await.unwrap()
    }

    #[tokio::test]
    async fn test_record_and_get_usage() {
        let store = make_store().await;
        store
            .record_usage("user-1", "sess-abc", 1000, "claude-sonnet-4-6")
            .await
            .unwrap();
        store
            .record_usage("user-1", "sess-abc", 500, "claude-sonnet-4-6")
            .await
            .unwrap();

        let summary = store.get_session_usage("sess-abc").await.unwrap();
        assert_eq!(summary.total_tokens, 1500);
        assert_eq!(summary.request_count, 2);
        assert!(summary.total_cost_usd > 0.0);
    }

    #[tokio::test]
    async fn test_get_usage_empty_session() {
        let store = make_store().await;
        let summary = store.get_session_usage("sess-nonexistent").await.unwrap();
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.request_count, 0);
        assert_eq!(summary.total_cost_usd, 0.0);
    }

    #[tokio::test]
    async fn test_record_usage_unknown_model_zero_cost() {
        let store = make_store().await;
        store
            .record_usage("user-1", "sess-local", 2000, "llama3.2")
            .await
            .unwrap();

        let summary = store.get_session_usage("sess-local").await.unwrap();
        assert_eq!(summary.total_tokens, 2000);
        assert_eq!(summary.total_cost_usd, 0.0);
    }

    #[tokio::test]
    async fn test_usage_isolated_by_session() {
        let store = make_store().await;
        store
            .record_usage("user-1", "sess-1", 100, "gpt-4o")
            .await
            .unwrap();
        store
            .record_usage("user-1", "sess-2", 200, "gpt-4o")
            .await
            .unwrap();

        let s1 = store.get_session_usage("sess-1").await.unwrap();
        let s2 = store.get_session_usage("sess-2").await.unwrap();
        assert_eq!(s1.total_tokens, 100);
        assert_eq!(s2.total_tokens, 200);
    }

    #[tokio::test]
    async fn test_record_usage_full_persists_breakdown() {
        let store = make_store().await;
        store
            .record_usage_full(
                "user-1",
                "sess-cache",
                1500,
                "claude-sonnet-4-6",
                UsageBreakdown {
                    input_tokens: Some(200),
                    output_tokens: Some(100),
                    cache_read_tokens: Some(1000),
                    cache_creation_tokens: Some(200),
                },
            )
            .await
            .unwrap();
        store
            .record_usage_full(
                "user-1",
                "sess-cache",
                500,
                "claude-sonnet-4-6",
                UsageBreakdown {
                    input_tokens: Some(150),
                    output_tokens: Some(50),
                    cache_read_tokens: Some(300),
                    cache_creation_tokens: None,
                },
            )
            .await
            .unwrap();

        let summary = store.get_session_usage("sess-cache").await.unwrap();
        assert_eq!(summary.total_tokens, 2000);
        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.total_input_tokens, 350);
        assert_eq!(summary.total_output_tokens, 150);
        assert_eq!(summary.total_cache_read_tokens, 1300);
        assert_eq!(summary.total_cache_creation_tokens, 200);
    }

    #[tokio::test]
    async fn test_get_total_usage_aggregates_across_sessions() {
        let store = make_store().await;
        store
            .record_usage_full(
                "user-1",
                "sess-a",
                400,
                "claude-sonnet-4-6",
                UsageBreakdown {
                    cache_read_tokens: Some(300),
                    ..UsageBreakdown::default()
                },
            )
            .await
            .unwrap();
        store
            .record_usage_full(
                "user-2",
                "sess-b",
                600,
                "gpt-4o",
                UsageBreakdown {
                    cache_read_tokens: Some(100),
                    ..UsageBreakdown::default()
                },
            )
            .await
            .unwrap();

        let summary = store.get_total_usage().await.unwrap();
        assert_eq!(summary.total_tokens, 1000);
        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.total_cache_read_tokens, 400);
    }

    #[tokio::test]
    async fn test_record_usage_wrapper_leaves_breakdown_null() {
        let store = make_store().await;
        store
            .record_usage("user-1", "sess-plain", 700, "gpt-4o")
            .await
            .unwrap();

        let summary = store.get_session_usage("sess-plain").await.unwrap();
        assert_eq!(summary.total_tokens, 700);
        // No breakdown was provided — aggregates remain at zero (NULLs sum to 0).
        assert_eq!(summary.total_input_tokens, 0);
        assert_eq!(summary.total_output_tokens, 0);
        assert_eq!(summary.total_cache_read_tokens, 0);
        assert_eq!(summary.total_cache_creation_tokens, 0);
    }
}
