//! Chat Completions wire format shared by the OpenAI-compatible providers
//! (OpenAI, OpenRouter and the named compatible providers), and the
//! per-model rules OpenAI documents for that endpoint.

use kernex_core::context::ApiMessage;
use serde::{Deserialize, Serialize};

use crate::tools::ToolDef;

// --- Serde types ---

/// Build OpenAI-format messages from context (system as a message role).
pub(crate) fn build_openai_messages(system: &str, api_messages: &[ApiMessage]) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(api_messages.len() + 1);
    if !system.is_empty() {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: Some(system.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    for m in api_messages {
        messages.push(ChatMessage {
            role: m.role.clone(),
            content: Some(m.content.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    messages
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMsg>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct ToolCallMsg {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
pub(crate) struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAiToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
}

#[derive(Serialize, Clone)]
pub(crate) struct OpenAiToolDef {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAiFunctionDef,
}

#[derive(Serialize, Clone)]
pub(crate) struct OpenAiFunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Deserialize)]
pub(crate) struct ChatCompletionResponse {
    pub choices: Option<Vec<ChatChoice>>,
    pub model: Option<String>,
    pub usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
pub(crate) struct ChatChoice {
    pub message: Option<ChatMessage>,
}

#[derive(Deserialize)]
pub(crate) struct ChatUsage {
    pub total_tokens: Option<u64>,
}

// --- Helper: convert ToolDef -> OpenAI format ---

pub(crate) fn to_openai_tools(defs: &[ToolDef]) -> Vec<OpenAiToolDef> {
    defs.iter()
        .map(|d| OpenAiToolDef {
            tool_type: "function".to_string(),
            function: OpenAiFunctionDef {
                name: d.name.clone(),
                description: d.description.clone(),
                parameters: d.parameters.clone(),
            },
        })
        .collect()
}

// --- Per-model rules for OpenAI's own models ---

/// The `reasoning_effort` to send with a tool-carrying Chat Completions
/// request.
///
/// OpenAI's GPT-6 Luna calls tools on Chat Completions only with
/// `reasoning_effort` set to `none`, and its default is `medium`, so a
/// request that omits it fails as soon as tools are offered. Every other
/// provider and model gets `None`, which leaves the field out of the body.
pub(crate) fn chat_tool_effort(provider_name: &str, model: &str) -> Option<&'static str> {
    (provider_name == "openai" && model.starts_with("gpt-6-luna")).then_some("none")
}

/// Whether OpenAI documents this model as calling tools only through the
/// Responses API (GPT-6.2 Sol and Astra, GPT-6.1 Terra).
pub(crate) fn needs_responses_for_tools(model: &str) -> bool {
    model.starts_with("gpt-6.2-") || model.starts_with("gpt-6.1-terra")
}

/// Whether `base_url` points at OpenAI's own API host: `api.openai.com`
/// or a regional host under it, such as `eu.api.openai.com`.
///
/// The `openai` provider is also used against proxies and compatible
/// servers; OpenAI's per-model rules are enforced only for OpenAI itself.
pub(crate) fn is_openai_host(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "api.openai.com" || h.ends_with(".api.openai.com"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_openai_luna_gets_an_effort() {
        assert_eq!(chat_tool_effort("openai", "gpt-6-luna"), Some("none"));
        assert_eq!(
            chat_tool_effort("openai", "gpt-6-luna-2026-10-01"),
            Some("none")
        );
        assert_eq!(chat_tool_effort("openai", "gpt-6.2-sol"), None);
        assert_eq!(chat_tool_effort("openai", "gpt-4o"), None);
        assert_eq!(chat_tool_effort("openrouter", "gpt-6-luna"), None);
        assert_eq!(chat_tool_effort("groq", "gpt-6-luna"), None);
    }

    #[test]
    fn responses_only_models() {
        assert!(needs_responses_for_tools("gpt-6.2-sol"));
        assert!(needs_responses_for_tools("gpt-6.2-astra"));
        assert!(needs_responses_for_tools("gpt-6.1-terra"));
        assert!(!needs_responses_for_tools("gpt-6-luna"));
        assert!(!needs_responses_for_tools("gpt-4o"));
    }

    #[test]
    fn openai_host_is_matched_by_host_only() {
        assert!(is_openai_host("https://api.openai.com/v1"));
        assert!(is_openai_host("https://API.OpenAI.com/v1/"));
        assert!(is_openai_host("https://eu.api.openai.com/v1"));
        assert!(!is_openai_host("https://evilapi.openai.com/v1"));
        assert!(!is_openai_host("https://api.openai.com.evil.test/v1"));
        assert!(!is_openai_host("http://localhost:4000/v1"));
        assert!(!is_openai_host("https://openrouter.ai/api/v1"));
        assert!(!is_openai_host("not a url"));
    }
}
