//! AWS Bedrock provider with SigV4 authentication.
//!
//! Supports Claude models on AWS Bedrock using the Anthropic Bedrock API format.
//! Authenticates with AWS SigV4 signatures using credentials from the environment
//! or explicit configuration.
//!
//! # Credentials
//!
//! Via standard AWS environment variables:
//! - `AWS_ACCESS_KEY_ID`
//! - `AWS_SECRET_ACCESS_KEY`
//! - `AWS_SESSION_TOKEN` (optional, for temporary / assumed-role credentials)
//! - `AWS_REGION` or `AWS_DEFAULT_REGION`
//!
//! # Model IDs
//!
//! Standard Bedrock model IDs for Claude:
//! - `anthropic.claude-3-5-sonnet-20241022-v2:0`
//! - `anthropic.claude-3-7-sonnet-20250219-v1:0`
//! - `us.anthropic.claude-sonnet-4-20250514-v1:0` (cross-region inference)

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use kernex_core::{context::Context, error::KernexError, message::Response, traits::Provider};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, warn};

use crate::http_retry::send_with_retry;
use crate::tools::build_response;

type HmacSha256 = Hmac<Sha256>;

const BEDROCK_SERVICE: &str = "bedrock";
const ANTHROPIC_VERSION_BEDROCK: &str = "bedrock-2023-05-31";
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// AWS Bedrock provider - Claude models via the Anthropic Bedrock API.
pub struct BedrockProvider {
    client: reqwest::Client,
    region: String,
    access_key_id: SecretString,
    secret_access_key: SecretString,
    session_token: Option<SecretString>,
    model_id: String,
    max_tokens: u32,
    #[allow(dead_code)]
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl BedrockProvider {
    /// Create from explicit credentials.
    #[allow(clippy::too_many_arguments)]
    pub fn from_config(
        region: String,
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
        model_id: String,
        max_tokens: u32,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| {
                    KernexError::Provider(format!("bedrock: failed to build HTTP client: {e}"))
                })?,
            region,
            access_key_id: SecretString::new(access_key_id),
            secret_access_key: SecretString::new(secret_access_key),
            session_token: session_token.map(SecretString::new),
            model_id,
            max_tokens,
            workspace_path,
            sandbox_profile: Default::default(),
        })
    }

    /// Create from standard AWS environment variables.
    ///
    /// Reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`,
    /// and `AWS_REGION` (falling back to `AWS_DEFAULT_REGION`).
    pub fn from_env(
        model_id: String,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        let access_key = std::env::var("AWS_ACCESS_KEY_ID")
            .map_err(|_| KernexError::Config("bedrock: AWS_ACCESS_KEY_ID not set".into()))?;
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
            .map_err(|_| KernexError::Config("bedrock: AWS_SECRET_ACCESS_KEY not set".into()))?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .map_err(|_| {
                KernexError::Config("bedrock: AWS_REGION or AWS_DEFAULT_REGION not set".into())
            })?;

        Self::from_config(
            region,
            access_key,
            secret_key,
            session_token,
            model_id,
            DEFAULT_MAX_TOKENS,
            workspace_path,
        )
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }
}

// --- SigV4 signing ---

/// Percent-encode a string for use in a SigV4 canonical URI.
///
/// Unreserved characters per RFC 3986 (`A-Z a-z 0-9 - _ . ~`) are kept
/// as-is. `/` is preserved when `encode_slash` is false (path separators).
/// Everything else is percent-encoded as `%XX` (uppercase hex).
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b'/' if !encode_slash => {
                out.push('/');
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// SHA-256 of `data` as a lowercase hex string.
fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 of `data` with `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, KernexError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|e| KernexError::Provider(format!("bedrock: HMAC init failed: {e}")))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

/// Hex-encode a byte slice.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Derive the SigV4 signing key from the secret access key.
fn derive_signing_key(
    secret: &str,
    date: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, KernexError> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let k_region = hmac_sha256(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

/// Build SigV4 authorization headers for a single POST request.
///
/// Returns `(authorization, x-amz-date, Option<x-amz-security-token>)`.
#[allow(clippy::too_many_arguments)]
fn sigv4_sign(
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
    region: &str,
    service: &str,
    path: &str,
    host: &str,
    body: &[u8],
) -> Result<(String, String, Option<String>), KernexError> {
    let now = chrono::Utc::now();
    let datetime = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();

    let canonical_uri = uri_encode(path, false);

    // Canonical headers — must be sorted by name (lowercase).
    let mut header_pairs: Vec<(&str, String)> = vec![
        ("content-type", "application/json".to_string()),
        ("host", host.to_string()),
        ("x-amz-date", datetime.clone()),
    ];
    if let Some(token) = session_token {
        header_pairs.push(("x-amz-security-token", token.to_string()));
    }
    header_pairs.sort_by_key(|(k, _)| *k);

    let canonical_headers: String = header_pairs
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();
    let signed_headers: String = header_pairs
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(";");

    let payload_hash = sha256_hex(body);
    let canonical_request =
        format!("POST\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    let credential_scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{datetime}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(secret_key, &date, region, service)?;
    let signature = to_hex(&hmac_sha256(&signing_key, string_to_sign.as_bytes())?);

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    Ok((authorization, datetime, session_token.map(String::from)))
}

// --- Anthropic Bedrock request/response types ---

/// Bedrock invoke body for Claude models.
#[derive(Serialize)]
struct BedrockClaudeRequest {
    anthropic_version: &'static str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<BedrockMessage>,
}

#[derive(Serialize)]
struct BedrockMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct BedrockClaudeResponse {
    content: Option<Vec<BedrockContentBlock>>,
    model: Option<String>,
    usage: Option<BedrockUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum BedrockContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct BedrockUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[async_trait]
impl Provider for BedrockProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model_id);

        let messages: Vec<BedrockMessage> = api_messages
            .iter()
            .map(|m| BedrockMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let system_opt = if system.is_empty() {
            None
        } else {
            Some(system)
        };

        let body = BedrockClaudeRequest {
            anthropic_version: ANTHROPIC_VERSION_BEDROCK,
            max_tokens: self.max_tokens,
            system: system_opt,
            messages,
        };

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("bedrock: serialize failed: {e}")))?;

        let host = format!("bedrock-runtime.{}.amazonaws.com", self.region);
        // Percent-encode the model ID (colon in version suffix must be encoded).
        let encoded_model = uri_encode(effective_model, true);
        let path = format!("/model/{encoded_model}/invoke");
        let url = format!("https://{host}{path}");

        debug!("bedrock: POST {url}");

        let start = Instant::now();

        let (auth_header, amz_date, maybe_token) = sigv4_sign(
            self.access_key_id.expose_secret(),
            self.secret_access_key.expose_secret(),
            self.session_token
                .as_ref()
                .map(|t| t.expose_secret().as_str()),
            &self.region,
            BEDROCK_SERVICE,
            &path,
            &host,
            &body_json,
        )?;

        let resp = {
            let client = &self.client;
            let body = body_json.clone();
            send_with_retry("bedrock", || {
                let mut req = client
                    .post(&url)
                    .header("content-type", "application/json")
                    .header("x-amz-date", &amz_date)
                    .header("authorization", &auth_header);
                if let Some(ref token) = maybe_token {
                    req = req.header("x-amz-security-token", token);
                }
                let req = req.body(body.clone());
                async move { req.send().await }
            })
            .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "bedrock returned {status}: {text}"
            )));
        }

        let parsed: BedrockClaudeResponse = resp.json().await.map_err(|e| {
            KernexError::Provider(format!("bedrock: failed to parse response: {e}"))
        })?;

        let text = parsed
            .content
            .as_ref()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| match b {
                        BedrockContentBlock::Text { text } => Some(text.as_str()),
                        BedrockContentBlock::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "No response from Bedrock.".to_string());

        let tokens = parsed
            .usage
            .as_ref()
            .map(|u| u.input_tokens + u.output_tokens)
            .unwrap_or(0);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(
            text,
            "bedrock",
            tokens,
            elapsed_ms,
            parsed.model,
        ))
    }

    async fn is_available(&self) -> bool {
        if self.access_key_id.expose_secret().is_empty() {
            warn!("bedrock: AWS_ACCESS_KEY_ID is empty");
            return false;
        }
        if self.secret_access_key.expose_secret().is_empty() {
            warn!("bedrock: AWS_SECRET_ACCESS_KEY is empty");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_and_requires_key() {
        let p = BedrockProvider::from_config(
            "us-east-1".into(),
            "AKIAIOSFODNN7EXAMPLE".into(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            None,
            "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
            DEFAULT_MAX_TOKENS,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "bedrock");
        assert!(p.requires_api_key());
    }

    #[test]
    fn uri_encode_preserves_unreserved() {
        assert_eq!(uri_encode("abc-123.~_", true), "abc-123.~_");
    }

    #[test]
    fn uri_encode_encodes_colon() {
        let model = "anthropic.claude-3-5-sonnet-20241022-v2:0";
        let encoded = uri_encode(model, true);
        assert_eq!(encoded, "anthropic.claude-3-5-sonnet-20241022-v2%3A0");
    }

    #[test]
    fn uri_encode_preserves_slash_when_not_encoding() {
        assert_eq!(
            uri_encode("/model/foo:0/invoke", false),
            "/model/foo%3A0/invoke"
        );
    }

    #[test]
    fn sha256_hex_known_value() {
        // echo -n "" | sha256sum => e3b0c44298fc1c14...
        let empty_hash = sha256_hex(b"");
        assert_eq!(
            empty_hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_sha256_produces_bytes() {
        let result = hmac_sha256(b"key", b"data");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn sigv4_sign_returns_valid_auth_header() {
        let (auth, amz_date, token) = sigv4_sign(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            None,
            "us-east-1",
            "bedrock",
            "/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke",
            "bedrock-runtime.us-east-1.amazonaws.com",
            b"{\"anthropic_version\":\"bedrock-2023-05-31\",\"max_tokens\":8192,\"messages\":[]}",
        )
        .unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"));
        assert!(auth.contains("Signature="));
        assert!(amz_date.ends_with('Z'));
        assert_eq!(amz_date.len(), 16); // YYYYMMDDTHHMMSSZassert!(token.is_none());
    }

    #[test]
    fn sigv4_sign_includes_session_token_header() {
        let (auth, _, token) = sigv4_sign(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            Some("session-token-xyz"),
            "us-east-1",
            "bedrock",
            "/model/foo/invoke",
            "bedrock-runtime.us-east-1.amazonaws.com",
            b"{}",
        )
        .unwrap();
        assert!(auth.contains("x-amz-security-token"));
        assert_eq!(token.as_deref(), Some("session-token-xyz"));
    }

    #[test]
    fn bedrock_request_serializes_without_system_when_empty() {
        let body = BedrockClaudeRequest {
            anthropic_version: ANTHROPIC_VERSION_BEDROCK,
            max_tokens: 8192,
            system: None,
            messages: vec![BedrockMessage {
                role: "user".into(),
                content: "Hello".into(),
            }],
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("system").is_none());
        assert_eq!(json["anthropic_version"], "bedrock-2023-05-31");
    }

    #[test]
    fn bedrock_request_includes_system_when_set() {
        let body = BedrockClaudeRequest {
            anthropic_version: ANTHROPIC_VERSION_BEDROCK,
            max_tokens: 8192,
            system: Some("Be helpful.".into()),
            messages: vec![],
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["system"], "Be helpful.");
    }

    #[test]
    fn bedrock_response_parsing() {
        let raw = r#"{"content":[{"type":"text","text":"Hi!"}],"model":"anthropic.claude-3-5-sonnet-20241022-v2:0","usage":{"input_tokens":5,"output_tokens":3}}"#;
        let resp: BedrockClaudeResponse = serde_json::from_str(raw).unwrap();
        let tokens = resp
            .usage
            .as_ref()
            .map(|u| u.input_tokens + u.output_tokens);
        assert_eq!(tokens, Some(8));
        let text = resp
            .content
            .unwrap()
            .iter()
            .filter_map(|b| match b {
                BedrockContentBlock::Text { text } => Some(text.as_str()),
                BedrockContentBlock::Other => None,
            })
            .collect::<String>();
        assert_eq!(text, "Hi!");
    }
}
