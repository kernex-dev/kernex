//! HTTP retry with exponential backoff for transient failures.
//!
//! Retries on network errors, 429 (rate limit), and 5xx (server errors).
//! Non-retryable errors (4xx except 429) are returned immediately.

use kernex_core::error::KernexError;
use reqwest::Response;
use std::future::Future;
use std::time::Duration;
use tracing::warn;

const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;

/// Execute an HTTP request with exponential backoff retry.
///
/// `build_request` is called on each attempt to produce a fresh `RequestBuilder`
/// (since reqwest consumes the builder on `.send()`).
///
/// Retries on:
/// - Network/connection errors
/// - HTTP 429 (rate limited)
/// - HTTP 5xx (server errors)
pub async fn send_with_retry<F, Fut>(
    provider_name: &str,
    build_request: F,
) -> Result<Response, KernexError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Response, reqwest::Error>>,
{
    let mut last_err = None;

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = Duration::from_millis(BASE_DELAY_MS * 2u64.pow(attempt - 1));
            warn!(
                "{provider_name}: retry {attempt}/{MAX_RETRIES} after {}ms",
                delay.as_millis()
            );
            tokio::time::sleep(delay).await;
        }

        match (build_request)().await {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    let body = resp.text().await.unwrap_or_default();
                    last_err = Some(format!("{provider_name} returned {status}: {body}"));
                    continue;
                }
                return Ok(resp);
            }
            Err(e) => {
                last_err = Some(format!("{provider_name} request failed: {e}"));
                continue;
            }
        }
    }

    Err(KernexError::Provider(last_err.unwrap_or_else(|| {
        format!("{provider_name}: request failed after retries")
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn retry_returns_success_on_first_try() {
        let count = Arc::new(AtomicU32::new(0));
        let c = count.clone();

        let resp = send_with_retry("test", || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(reqwest::Response::from(
                    http::Response::builder().status(200).body("").unwrap(),
                ))
            }
        })
        .await;

        assert!(resp.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_retries_on_server_error() {
        let count = Arc::new(AtomicU32::new(0));
        let c = count.clone();

        let resp = send_with_retry("test", || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let status = if n < 2 { 500 } else { 200 };
                Ok(reqwest::Response::from(
                    http::Response::builder().status(status).body("").unwrap(),
                ))
            }
        })
        .await;

        assert!(resp.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
