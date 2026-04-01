//! Token usage recording and cost tracking.

use super::Store;
use kernex_core::error::KernexError;
use kernex_core::pricing::pricing_for;

/// Aggregated token usage for a session or sender.
#[derive(Debug, Clone, Default)]
pub struct UsageSummary {
    /// Total tokens consumed across all recorded requests.
    pub total_tokens: i64,
    /// Estimated total cost in USD.
    pub total_cost_usd: f64,
    /// Number of API requests recorded.
    pub request_count: i64,
}

impl Store {
    /// Record token usage for a completed API request.
    ///
    /// Cost is estimated using known per-model pricing. If the model is
    /// unrecognized, cost is recorded as 0.0.
    pub async fn record_usage(
        &self,
        sender_id: &str,
        session_id: &str,
        tokens: u64,
        model: &str,
    ) -> Result<(), KernexError> {
        let cost = pricing_for(model)
            .map(|p| p.estimate_cost(tokens))
            .unwrap_or(0.0);

        sqlx::query(
            "INSERT INTO token_usage (sender_id, session_id, model, tokens, cost_usd)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(sender_id)
        .bind(session_id)
        .bind(model)
        .bind(tokens as i64)
        .bind(cost)
        .execute(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("failed to record token usage: {e}")))?;

        Ok(())
    }

    /// Get aggregated token usage for a session.
    pub async fn get_session_usage(&self, session_id: &str) -> Result<UsageSummary, KernexError> {
        let row: Option<(i64, f64, i64)> = sqlx::query_as(
            "SELECT COALESCE(SUM(tokens), 0), COALESCE(SUM(cost_usd), 0.0), COUNT(*)
             FROM token_usage WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernexError::Store(format!("failed to query session usage: {e}")))?;

        let (total_tokens, total_cost_usd, request_count) = row.unwrap_or((0, 0.0, 0));

        Ok(UsageSummary {
            total_tokens,
            total_cost_usd,
            request_count,
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
}
