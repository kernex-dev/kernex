//! OpenAI-compatible API provider.
//!
//! Works with OpenAI's API and any compatible endpoint.
//! Exports `pub(crate)` types and the agentic loop reused by the OpenRouter provider.

use async_trait::async_trait;
use kernex_core::{
    context::{ApiMessage, Context},
    error::KernexError,
    message::Response,
    traits::Provider,
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::send_with_retry;
use crate::tools::{build_response, tools_enabled, ToolDef, ToolExecutor};

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// OpenAI-compatible provider with tool-execution loop.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    model: String,
    name: String,
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl OpenAiProvider {
    /// Create from config values.
    pub fn from_config(
        base_url: String,
        api_key: String,
        model: String,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| KernexError::Provider(format!("failed to build HTTP client: {e}")))?,
            base_url,
            api_key: SecretString::new(api_key),
            model,
            name: "openai".to_string(),
            workspace_path,
            sandbox_profile: Default::default(),
        })
    }

    /// Override the provider name reported by [`Provider::name`].
    ///
    /// Used by the factory to give OpenAI-compatible variants (groq, mistral,
    /// deepseek, etc.) their own identity for logging and metrics.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }
}

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

// --- Shared agentic loop ---

/// Run the OpenAI-format agentic loop (used by OpenAI and OpenRouter).
///
/// Loops: infer -> tool calls -> execute -> feed results back, until the model
/// produces a text-only response or max_turns is reached.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn openai_agentic_complete(
    client: &reqwest::Client,
    url: &str,
    auth_header: &str,
    model: &str,
    system: &str,
    api_messages: &[ApiMessage],
    executor: &mut ToolExecutor,
    max_turns: u32,
    provider_name: &str,
) -> Result<Response, KernexError> {
    let start = Instant::now();

    // Build initial messages and tools.
    let mut messages = build_openai_messages(system, api_messages);
    let all_tool_defs = executor.all_tool_defs();
    let tools = if all_tool_defs.is_empty() {
        None
    } else {
        Some(to_openai_tools(&all_tool_defs))
    };

    let mut last_model: Option<String> = None;
    let mut total_tokens: u64 = 0;

    for turn in 0..max_turns {
        let body = ChatCompletionRequest {
            model: model.to_string(),
            messages: messages.clone(),
            tools: tools.clone(),
        };

        debug!("{provider_name}: POST {url} model={model} turn={turn}");

        let body_json = serde_json::to_vec(&body).map_err(|e| {
            KernexError::Provider(format!("{provider_name}: serialize failed: {e}"))
        })?;

        let resp = send_with_retry(provider_name, || {
            let req = client
                .post(url)
                .header("Authorization", auth_header)
                .header("Content-Type", "application/json")
                .body(body_json.clone());
            async move { req.send().await }
        })
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "{provider_name} returned {status}: {text}"
            )));
        }

        let parsed: ChatCompletionResponse = resp.json().await.map_err(|e| {
            KernexError::Provider(format!("{provider_name}: failed to parse response: {e}"))
        })?;

        if let Some(ref m) = parsed.model {
            last_model = Some(m.clone());
        }
        if let Some(ref u) = parsed.usage {
            if let Some(t) = u.total_tokens {
                total_tokens += t;
            }
        }

        let choice = parsed
            .choices
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.message.clone());

        let Some(assistant_msg) = choice else {
            break;
        };

        // Check for tool calls.
        if let Some(ref tool_calls) = assistant_msg.tool_calls {
            if !tool_calls.is_empty() {
                // Append the assistant message (with tool_calls) to the conversation.
                messages.push(assistant_msg.clone());

                for tc in tool_calls {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                    info!(
                        "{provider_name}: tool call [{turn}] {} ({})",
                        tc.function.name, tc.id
                    );

                    let result = executor.execute(&tc.function.name, &args).await;

                    // Append tool result message.
                    messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(result.content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                    });
                }

                continue; // Next turn.
            }
        }

        // Text-only response — we're done.
        let text = assistant_msg
            .content
            .unwrap_or_else(|| format!("No response from {provider_name}."));

        let elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(build_response(
            text,
            provider_name,
            total_tokens,
            elapsed_ms,
            last_model,
        ));
    }

    // Max turns exhausted.
    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(build_response(
        format!("{provider_name}: reached max turns ({max_turns}) without final response"),
        provider_name,
        total_tokens,
        elapsed_ms,
        last_model,
    ))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        let name = self.name.as_str();
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let auth = format!("Bearer {}", self.api_key.expose_secret());
        let max_turns = context.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone())
                    .with_hook_runner_opt(context.hook_runner.clone())
                    .with_permission_rules_opt(context.permission_rules.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let result = openai_agentic_complete(
                    &self.client,
                    &url,
                    &auth,
                    effective_model,
                    &system,
                    &api_messages,
                    &mut executor,
                    max_turns,
                    name,
                )
                .await;

                executor.shutdown_mcp().await;
                return result;
            }
        }

        // Fallback: no tools (classification calls, or no workspace).
        let start = Instant::now();
        let messages = build_openai_messages(&system, &api_messages);
        let body = ChatCompletionRequest {
            model: effective_model.to_string(),
            messages,
            tools: None,
        };

        debug!("{name}: POST {url} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("{name}: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let url = &url;
            let auth = &auth;
            send_with_retry(name, || {
                let req = client
                    .post(url.as_str())
                    .header("Authorization", auth.as_str())
                    .header("Content-Type", "application/json")
                    .body(body_json.clone());
                async move { req.send().await }
            })
            .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "{name} returned {status}: {text}"
            )));
        }

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .map_err(|e| KernexError::Provider(format!("{name}: failed to parse response: {e}")))?;

        let text = parsed
            .choices
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| format!("No response from {name}."));

        let tokens = parsed
            .usage
            .as_ref()
            .and_then(|u| u.total_tokens)
            .unwrap_or(0);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(text, name, tokens, elapsed_ms, parsed.model))
    }

    async fn is_available(&self) -> bool {
        let name = self.name.as_str();
        if self.api_key.expose_secret().is_empty() {
            warn!("{name}: no API key configured");
            return false;
        }
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        match self
            .client
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("{name} not available: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_provider_name() {
        let p = OpenAiProvider::from_config(
            "https://api.openai.com/v1".into(),
            "sk-test".into(),
            "gpt-4o".into(),
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "openai");
        assert!(p.requires_api_key());
    }

    #[test]
    fn test_with_name_override() {
        let p = OpenAiProvider::from_config(
            "https://api.groq.com/openai/v1".into(),
            "gsk_test".into(),
            "llama-3.3-70b-versatile".into(),
            None,
        )
        .unwrap()
        .with_name("groq");
        assert_eq!(p.name(), "groq");
        assert!(p.requires_api_key());
    }

    #[test]
    fn test_build_openai_messages() {
        let api_msgs = vec![
            ApiMessage {
                role: "user".into(),
                content: "Hi".into(),
            },
            ApiMessage {
                role: "assistant".into(),
                content: "Hello!".into(),
            },
            ApiMessage {
                role: "user".into(),
                content: "How?".into(),
            },
        ];
        let messages = build_openai_messages("Be helpful.", &api_msgs);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content.as_deref(), Some("Be helpful."));
        assert_eq!(messages[3].role, "user");
    }

    #[test]
    fn test_build_openai_messages_empty_system() {
        let api_msgs = vec![ApiMessage {
            role: "user".into(),
            content: "Hi".into(),
        }];
        let messages = build_openai_messages("", &api_msgs);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn test_openai_response_parsing() {
        let json = r#"{"choices":[{"message":{"role":"assistant","content":"Hello!"},"finish_reason":"stop"}],"model":"gpt-4o","usage":{"total_tokens":42,"prompt_tokens":10,"completion_tokens":32}}"#;
        let resp: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let text = resp
            .choices
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone());
        assert_eq!(text, Some("Hello!".into()));
        assert_eq!(resp.usage.as_ref().and_then(|u| u.total_tokens), Some(42));
    }

    #[test]
    fn test_openai_tool_call_response_parsing() {
        let json = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_123","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}],"model":"gpt-4o","usage":{"total_tokens":50}}"#;
        let resp: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let msg = resp
            .choices
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .message
            .as_ref()
            .unwrap();
        assert!(msg.content.is_none());
        let tcs = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "bash");
        assert_eq!(tcs[0].id, "call_123");
    }

    #[test]
    fn test_to_openai_tools() {
        let defs = crate::tools::builtin_tool_defs();
        let tools = to_openai_tools(&defs);
        assert_eq!(tools.len(), 7);
        assert_eq!(tools[0].tool_type, "function");
        assert!(!tools[0].function.name.is_empty());
    }

    #[test]
    fn test_chat_completion_request_no_tools() {
        let req = ChatCompletionRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("Hi".into()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_chat_completion_request_with_tools() {
        let defs = crate::tools::builtin_tool_defs();
        let req = ChatCompletionRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("list files".into()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: Some(to_openai_tools(&defs)),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("tools").unwrap().as_array().unwrap().len() == 7);
    }
}
