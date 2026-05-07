//! HTTP retry with exponential backoff for transient failures.
//!
//! Retries on network errors, 429 (rate limit), and 5xx (server errors).
//! Non-retryable errors (4xx except 429) are returned immediately.

use futures_util::StreamExt;
use kernex_core::error::KernexError;
use reqwest::Response;
use std::future::Future;
use std::time::Duration;
use tracing::warn;

const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;
/// Upper bound on a server-supplied `Retry-After` value. Without a cap a
/// hostile or buggy provider could pin retries open for days; 30 s is long
/// enough to weather a real rate-limit window.
const MAX_RETRY_AFTER_SECS: u64 = 30;

/// Parse a `Retry-After` header value. RFC 7231 allows either a decimal
/// number of seconds or an HTTP-date; we honour the seconds form (the
/// common case in JSON APIs) and ignore the date form rather than pulling
/// in a date parser. Returns `None` for missing/invalid/overlong values.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = value.trim().parse().ok()?;
    let clamped = secs.min(MAX_RETRY_AFTER_SECS);
    Some(Duration::from_secs(clamped))
}

/// Maximum number of bytes read from a non-2xx response body when building an
/// error message. Keeps memory bounded under malicious or accidental floods
/// (e.g. a 5xx page that ships gigabytes of HTML) and truncates request-echo
/// content that some providers include in 4xx bodies, since system prompts
/// and tool inputs may contain user-supplied secrets.
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

/// Read up to 16 KB from a non-2xx response, returning a UTF-8 lossy String
/// suitable for inclusion in a [`KernexError::Provider`] message. Stops early
/// once the cap is reached, even if the server is still streaming. Appends
/// `" [... truncated]"` when the body exceeded the cap.
pub async fn read_truncated_error_body(resp: Response) -> String {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_ERROR_BODY_BYTES.min(8 * 1024));
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(_) => break,
        };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        if chunk.len() > remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        s.push_str(" [... truncated]");
    }
    s
}

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
    let mut next_delay: Option<Duration> = None;

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            // Honour Retry-After when the previous response provided it; fall
            // back to exponential backoff. We pick the larger of the two so a
            // server saying "wait 10s" is always respected, but a server
            // omitting Retry-After still gets a sensible delay.
            let backoff = Duration::from_millis(BASE_DELAY_MS * 2u64.pow(attempt - 1));
            let delay = next_delay.take().map(|d| d.max(backoff)).unwrap_or(backoff);
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
                    next_delay = parse_retry_after(resp.headers());
                    let body = read_truncated_error_body(resp).await;
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

    #[test]
    fn parse_retry_after_seconds_form() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_clamps_overlong_values() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "86400".parse().unwrap());
        assert_eq!(
            parse_retry_after(&h),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
    }

    #[test]
    fn parse_retry_after_ignores_http_date_form() {
        // We don't pull in a date parser; HTTP-date Retry-After becomes None
        // and we fall back to exponential backoff.
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn parse_retry_after_missing_returns_none() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
    }

    #[tokio::test]
    async fn read_truncated_error_body_caps_long_payload() {
        let big = "X".repeat(MAX_ERROR_BODY_BYTES * 2);
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(500)
                .body(big.clone())
                .unwrap(),
        );
        let body = read_truncated_error_body(resp).await;
        // Truncation marker present and prefix is exactly the cap.
        assert!(body.ends_with(" [... truncated]"), "missing marker: {body}");
        let prefix_len = body.len() - " [... truncated]".len();
        assert_eq!(prefix_len, MAX_ERROR_BODY_BYTES);
        assert!(body.starts_with("XXXX"));
    }

    #[tokio::test]
    async fn read_truncated_error_body_passes_short_payload() {
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(400)
                .body("bad request: missing model")
                .unwrap(),
        );
        let body = read_truncated_error_body(resp).await;
        assert_eq!(body, "bad request: missing model");
    }

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
