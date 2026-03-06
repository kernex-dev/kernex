//! Request and response types for the Kernex runtime.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An incoming request to the runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: Uuid,
    pub sender_id: String,
    #[serde(default)]
    pub sender_name: Option<String>,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub reply_to: Option<Uuid>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub source: Option<String>,
}

impl Request {
    /// Create a simple text request.
    pub fn text(sender_id: &str, text: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            sender_id: sender_id.to_string(),
            sender_name: None,
            text: text.to_string(),
            timestamp: Utc::now(),
            reply_to: None,
            attachments: Vec::new(),
            source: None,
        }
    }
}

/// A response from an AI provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub text: String,
    #[serde(default)]
    pub metadata: CompletionMeta,
}

/// Metadata about how a completion was generated.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionMeta {
    pub provider_used: String,
    #[serde(default)]
    pub tokens_used: Option<u64>,
    pub processing_time_ms: u64,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A file attachment on a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub file_type: AttachmentType,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub data: Option<Vec<u8>>,
    #[serde(default)]
    pub filename: Option<String>,
}

/// Supported attachment types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttachmentType {
    Image,
    Document,
    Audio,
    Video,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_text_constructor() {
        let req = Request::text("user-1", "hello");
        assert_eq!(req.sender_id, "user-1");
        assert_eq!(req.text, "hello");
        assert!(req.attachments.is_empty());
    }

    #[test]
    fn test_response_default() {
        let resp = Response::default();
        assert!(resp.text.is_empty());
        assert!(resp.metadata.provider_used.is_empty());
    }

    #[test]
    fn test_request_serde_round_trip() {
        let req = Request::text("user-1", "test message");
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.sender_id, "user-1");
        assert_eq!(deserialized.text, "test message");
    }

    #[test]
    fn test_completion_meta_session_id_skipped_when_none() {
        let meta = CompletionMeta::default();
        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("session_id"));
    }
}
