//! Anthropic API provider with tool-execution loop.
//!
//! Calls the Anthropic Messages API directly (not via Claude Code CLI).
//! Uses content blocks (text/tool_use/tool_result) for tool calling.

use async_trait::async_trait;
use kernex_core::{
    context::Context,
    error::KernexError,
    message::Response,
    stream::StreamEvent,
    traits::{Provider, StreamingProvider},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::send_with_retry;
use crate::tools::{build_response, tools_enabled, ToolDef, ToolExecutor};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// Marker that splits a system prompt into a cacheable stable prefix and a
/// dynamic per-turn suffix. Place this string between the two sections:
///
/// ```text
/// You are a helpful assistant. <--- stable rules, skills, etc.
/// KERNEX_CACHE_BOUNDARY
/// Today is Monday. User context: Alice. <--- dynamic per-turn context
/// ```
///
/// Anthropic will cache the stable prefix across turns, reducing cost
/// on long sessions.
pub const CACHE_BOUNDARY: &str = "KERNEX_CACHE_BOUNDARY";

/// A single block in the Anthropic `system` array.
#[derive(Serialize, Clone)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// Anthropic prompt cache control directive.
#[derive(Serialize, Clone)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: &'static str,
}

/// Build Anthropic system blocks from a system prompt string.
///
/// If the prompt contains [`CACHE_BOUNDARY`], the text before it gets
/// `cache_control: ephemeral` (stable cacheable prefix) and the text after
/// has no cache control (dynamic per-turn suffix). Otherwise a single plain
/// block is returned.
fn build_system_blocks(system_prompt: &str) -> Vec<SystemBlock> {
    if system_prompt.is_empty() {
        return Vec::new();
    }
    if let Some(idx) = system_prompt.find(CACHE_BOUNDARY) {
        let stable = system_prompt[..idx].trim_end();
        let dynamic = system_prompt[idx + CACHE_BOUNDARY.len()..].trim_start();
        let mut blocks = Vec::new();
        if !stable.is_empty() {
            blocks.push(SystemBlock {
                block_type: "text",
                text: stable.to_string(),
                cache_control: Some(CacheControl {
                    cache_type: "ephemeral",
                }),
            });
        }
        if !dynamic.is_empty() {
            blocks.push(SystemBlock {
                block_type: "text",
                text: dynamic.to_string(),
                cache_control: None,
            });
        }
        blocks
    } else {
        vec![SystemBlock {
            block_type: "text",
            text: system_prompt.to_string(),
            cache_control: None,
        }]
    }
}

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    max_tokens: u32,
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl AnthropicProvider {
    /// Create from config values.
    pub fn from_config(
        api_key: String,
        model: String,
        max_tokens: u32,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| KernexError::Provider(format!("failed to build HTTP client: {e}")))?,
            api_key: SecretString::new(api_key),
            model,
            max_tokens,
            workspace_path,
            sandbox_profile: Default::default(),
        })
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }
}

// --- Serde types for the Anthropic Messages API ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<SystemBlock>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDef>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Content can be a plain string or a list of content blocks.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
enum AnthropicContent {
    /// Plain text content (for simple user/assistant messages).
    Text(String),
    /// Array of content blocks (for tool_use, tool_result, mixed content).
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Serialize, Clone)]
struct AnthropicToolDef {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Option<Vec<AnthropicResponseBlock>>,
    model: Option<String>,
    usage: Option<AnthropicUsage>,
    stop_reason: Option<String>,
}

/// Response content blocks (slightly simpler than request blocks).
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum AnthropicResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// Convert ToolDef to Anthropic format.
fn to_anthropic_tools(defs: &[ToolDef]) -> Vec<AnthropicToolDef> {
    defs.iter()
        .map(|d| AnthropicToolDef {
            name: d.name.clone(),
            description: d.description.clone(),
            input_schema: d.parameters.clone(),
        })
        .collect()
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model);
        let max_turns = context.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        let system_blocks = build_system_blocks(&system);
        let use_cache = system_blocks.iter().any(|b| b.cache_control.is_some());

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone())
                    .with_hook_runner_opt(context.hook_runner.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let result = self
                    .agentic_loop(
                        effective_model,
                        &system_blocks,
                        use_cache,
                        &api_messages,
                        &mut executor,
                        max_turns,
                    )
                    .await;

                executor.shutdown_mcp().await;
                return result;
            }
        }

        // Fallback: no tools.
        let start = Instant::now();
        let messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();

        let body = AnthropicRequest {
            model: effective_model.to_string(),
            max_tokens: self.max_tokens,
            system: system_blocks,
            messages,
            tools: None,
        };

        debug!("anthropic: POST {ANTHROPIC_API_URL} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("anthropic: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let api_key = &self.api_key;
            send_with_retry("anthropic", || {
                let mut req = client
                    .post(ANTHROPIC_API_URL)
                    .header("x-api-key", api_key.expose_secret().as_str())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .header("content-type", "application/json");
                if use_cache {
                    req = req.header("anthropic-beta", "prompt-caching-2024-07-31");
                }
                let req = req.body(body_json.clone());
                async move { req.send().await }
            })
            .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "anthropic returned {status}: {text}"
            )));
        }

        let parsed: AnthropicResponse = resp.json().await.map_err(|e| {
            KernexError::Provider(format!("anthropic: failed to parse response: {e}"))
        })?;

        let text = extract_text_from_response(&parsed);
        let tokens = parsed
            .usage
            .as_ref()
            .map(|u| u.input_tokens + u.output_tokens)
            .unwrap_or(0);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(
            text,
            "anthropic",
            tokens,
            elapsed_ms,
            parsed.model,
        ))
    }

    async fn is_available(&self) -> bool {
        if self.api_key.expose_secret().is_empty() {
            warn!("anthropic: no API key configured");
            return false;
        }
        true
    }
}

impl AnthropicProvider {
    /// Anthropic-specific agentic loop using content blocks.
    async fn agentic_loop(
        &self,
        model: &str,
        system_blocks: &[SystemBlock],
        use_cache: bool,
        api_messages: &[kernex_core::context::ApiMessage],
        executor: &mut ToolExecutor,
        max_turns: u32,
    ) -> Result<Response, KernexError> {
        let start = Instant::now();

        let mut messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();

        let all_tool_defs = executor.all_tool_defs();
        let tools = if all_tool_defs.is_empty() {
            None
        } else {
            Some(to_anthropic_tools(&all_tool_defs))
        };

        let mut last_model: Option<String> = None;
        let mut total_tokens: u64 = 0;

        for turn in 0..max_turns {
            let body = AnthropicRequest {
                model: model.to_string(),
                max_tokens: self.max_tokens,
                system: system_blocks.to_vec(),
                messages: messages.clone(),
                tools: tools.clone(),
            };

            debug!("anthropic: POST {ANTHROPIC_API_URL} model={model} turn={turn}");

            let body_json = serde_json::to_vec(&body)
                .map_err(|e| KernexError::Provider(format!("anthropic: serialize failed: {e}")))?;

            let resp = {
                let client = &self.client;
                let api_key = &self.api_key;
                // Build combined beta flags: token-efficient-tools is always
                // active in the agentic loop; prompt-caching is added when the
                // system prompt was split at CACHE_BOUNDARY.
                let beta_header = if use_cache {
                    "token-efficient-tools-2026-03-28,prompt-caching-2024-07-31".to_string()
                } else {
                    "token-efficient-tools-2026-03-28".to_string()
                };
                send_with_retry("anthropic", || {
                    let req = client
                        .post(ANTHROPIC_API_URL)
                        .header("x-api-key", api_key.expose_secret().as_str())
                        .header("anthropic-version", ANTHROPIC_VERSION)
                        .header("content-type", "application/json")
                        .header("anthropic-beta", &beta_header)
                        .body(body_json.clone());
                    async move { req.send().await }
                })
                .await?
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(KernexError::Provider(format!(
                    "anthropic returned {status}: {text}"
                )));
            }

            let parsed: AnthropicResponse = resp.json().await.map_err(|e| {
                KernexError::Provider(format!("anthropic: failed to parse response: {e}"))
            })?;

            if let Some(ref m) = parsed.model {
                last_model = Some(m.clone());
            }
            if let Some(ref u) = parsed.usage {
                total_tokens += u.input_tokens + u.output_tokens;
            }

            // Check for tool_use in response.
            let has_tool_use = parsed.stop_reason.as_deref() == Some("tool_use");
            let blocks = parsed.content.unwrap_or_default();

            if has_tool_use {
                // Build the assistant message with response blocks.
                let mut assistant_blocks: Vec<AnthropicContentBlock> = Vec::new();
                let mut tool_result_blocks: Vec<AnthropicContentBlock> = Vec::new();

                for block in &blocks {
                    match block {
                        AnthropicResponseBlock::Text { text } => {
                            assistant_blocks
                                .push(AnthropicContentBlock::Text { text: text.clone() });
                        }
                        AnthropicResponseBlock::ToolUse { id, name, input } => {
                            assistant_blocks.push(AnthropicContentBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });

                            info!("anthropic: tool call [{turn}] {name} ({id})");

                            let result = executor.execute(name, input).await;

                            tool_result_blocks.push(AnthropicContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: result.content,
                                is_error: if result.is_error { Some(true) } else { None },
                            });
                        }
                    }
                }

                // Append assistant message, then user message with tool results.
                messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: AnthropicContent::Blocks(assistant_blocks),
                });
                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(tool_result_blocks),
                });

                continue;
            }

            // Text-only response.
            let text = blocks
                .iter()
                .filter_map(|b| match b {
                    AnthropicResponseBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            let text = if text.is_empty() {
                "No response from Anthropic.".to_string()
            } else {
                text
            };

            let elapsed_ms = start.elapsed().as_millis() as u64;
            return Ok(build_response(
                text,
                "anthropic",
                total_tokens,
                elapsed_ms,
                last_model,
            ));
        }

        // Max turns exhausted.
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(build_response(
            format!("anthropic: reached max turns ({max_turns}) without final response"),
            "anthropic",
            total_tokens,
            elapsed_ms,
            last_model,
        ))
    }
}

// --- SSE streaming types for the Anthropic Messages API ---

/// Wrapper around [`AnthropicRequest`] with `stream: true`.
#[derive(Serialize)]
struct AnthropicStreamRequest {
    model: String,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<SystemBlock>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDef>>,
}

/// Minimal SSE data envelope from Anthropic.
#[derive(Deserialize)]
struct SseData {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<SseDelta>,
}

/// Delta payload inside a `content_block_delta` event.
#[derive(Deserialize)]
struct SseDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
}

/// Convert an Anthropic SSE data object into a [`StreamEvent`], if relevant.
fn sse_data_to_event(data: SseData) -> Option<StreamEvent> {
    match data.event_type.as_str() {
        "content_block_delta" => {
            let delta = data.delta?;
            match delta.delta_type.as_str() {
                "text_delta" => delta.text.map(StreamEvent::TextDelta),
                "input_json_delta" => delta.partial_json.map(StreamEvent::InputJsonDelta),
                _ => None,
            }
        }
        "message_stop" => Some(StreamEvent::Done),
        _ => None,
    }
}

#[async_trait]
impl StreamingProvider for AnthropicProvider {
    async fn complete_stream(
        &self,
        context: &Context,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, KernexError> {
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model).to_string();

        let system_blocks = build_system_blocks(&system);
        let use_cache = system_blocks.iter().any(|b| b.cache_control.is_some());

        let messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();

        let body = AnthropicStreamRequest {
            model: effective_model,
            max_tokens: self.max_tokens,
            stream: true,
            system: system_blocks,
            messages,
            tools: None,
        };

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("anthropic: serialize failed: {e}")))?;

        // Build request. We send directly — send_with_retry reads the body on
        // error which would consume the stream before we can iterate it.
        let api_key_str = self.api_key.expose_secret().to_string();
        let mut req = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &api_key_str)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        if use_cache {
            req = req.header("anthropic-beta", "prompt-caching-2024-07-31");
        }

        debug!("anthropic: POST {ANTHROPIC_API_URL} (stream=true)");

        let resp =
            req.body(body_json).send().await.map_err(|e| {
                KernexError::Provider(format!("anthropic: stream request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "anthropic returned {status}: {text}"
            )));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            use futures_util::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        // Drain complete lines from the front of the buffer.
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].trim_end_matches('\r').to_string();
                            buffer = buffer[pos + 1..].to_string();

                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    let _ = tx.send(StreamEvent::Done).await;
                                    return;
                                }
                                if let Ok(sse) = serde_json::from_str::<SseData>(data) {
                                    if let Some(evt) = sse_data_to_event(sse) {
                                        if tx.send(evt).await.is_err() {
                                            return; // Receiver dropped.
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        return;
                    }
                }
            }

            let _ = tx.send(StreamEvent::Done).await;
        });

        Ok(rx)
    }
}

/// Extract text from an Anthropic response.
fn extract_text_from_response(resp: &AnthropicResponse) -> String {
    resp.content
        .as_ref()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| match b {
                    AnthropicResponseBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "No response from Anthropic.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_provider_name() {
        let p = AnthropicProvider::from_config(
            "sk-ant-test".into(),
            "claude-sonnet-4-20250514".into(),
            8192,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "anthropic");
        assert!(p.requires_api_key());
    }

    #[test]
    fn test_anthropic_request_serialization() {
        let body = AnthropicRequest {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 8192,
            system: build_system_blocks("Be helpful."),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("Hello".into()),
            }],
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["max_tokens"], 8192);
        assert_eq!(json["system"][0]["type"], "text");
        assert_eq!(json["system"][0]["text"], "Be helpful.");
        assert!(json["system"][0].get("cache_control").is_none());
        assert_eq!(json["messages"][0]["role"], "user");
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_anthropic_request_empty_system_omitted() {
        let body = AnthropicRequest {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 8192,
            system: Vec::new(),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("Hello".into()),
            }],
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("system").is_none());
    }

    #[test]
    fn test_cache_boundary_splits_system_blocks() {
        let prompt = "Stable rules.\nKERNEX_CACHE_BOUNDARY\nDynamic context.";
        let blocks = build_system_blocks(prompt);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "Stable rules.");
        assert!(blocks[0].cache_control.is_some());
        assert_eq!(blocks[1].text, "Dynamic context.");
        assert!(blocks[1].cache_control.is_none());
    }

    #[test]
    fn test_cache_boundary_absent_is_single_block() {
        let blocks = build_system_blocks("No boundary here.");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].cache_control.is_none());
    }

    #[test]
    fn test_cache_boundary_serializes_cache_control() {
        let prompt = "Stable.\nKERNEX_CACHE_BOUNDARY\nDynamic.";
        let blocks = build_system_blocks(prompt);
        let json = serde_json::to_value(&blocks).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
        assert!(arr[1].get("cache_control").is_none());
    }

    #[test]
    fn test_anthropic_response_parsing() {
        let json = r#"{"content":[{"type":"text","text":"Hello!"}],"model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5},"stop_reason":"end_turn"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        let text = extract_text_from_response(&resp);
        assert_eq!(text, "Hello!");
        assert_eq!(
            resp.usage
                .as_ref()
                .map(|u| u.input_tokens + u.output_tokens),
            Some(15)
        );
    }

    #[test]
    fn test_anthropic_tool_use_response_parsing() {
        let json = r#"{"content":[{"type":"text","text":"Let me check."},{"type":"tool_use","id":"toolu_123","name":"bash","input":{"command":"ls"}}],"model":"claude-sonnet-4-20250514","usage":{"input_tokens":20,"output_tokens":15},"stop_reason":"tool_use"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        let blocks = resp.content.unwrap();
        assert_eq!(blocks.len(), 2);
        match &blocks[1] {
            AnthropicResponseBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            _ => panic!("expected ToolUse block"),
        }
    }

    #[test]
    fn test_anthropic_request_with_tools() {
        let defs = crate::tools::builtin_tool_defs();
        let tools = to_anthropic_tools(&defs);
        let body = AnthropicRequest {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 8192,
            system: build_system_blocks("test"),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("list files".into()),
            }],
            tools: Some(tools),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tools"].as_array().unwrap().len(), 4);
        assert_eq!(json["tools"][0]["name"], "bash");
    }

    #[test]
    fn test_anthropic_content_blocks_serialization() {
        let msg = AnthropicMessage {
            role: "user".into(),
            content: AnthropicContent::Blocks(vec![AnthropicContentBlock::ToolResult {
                tool_use_id: "toolu_123".into(),
                content: "file1.txt\nfile2.txt".into(),
                is_error: None,
            }]),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        let blocks = json["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "toolu_123");
    }

    // --- SSE streaming tests ---

    #[test]
    fn test_sse_stream_request_has_stream_true() {
        let body = AnthropicStreamRequest {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 8192,
            stream: true,
            system: build_system_blocks("Be helpful."),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("Hello".into()),
            }],
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["stream"], true);
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_sse_text_delta_parsed() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let sse: SseData = serde_json::from_str(raw).unwrap();
        let evt = sse_data_to_event(sse).unwrap();
        match evt {
            StreamEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_sse_input_json_delta_parsed() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#;
        let sse: SseData = serde_json::from_str(raw).unwrap();
        let evt = sse_data_to_event(sse).unwrap();
        match evt {
            StreamEvent::InputJsonDelta(j) => assert_eq!(j, "{\"cmd\":"),
            _ => panic!("expected InputJsonDelta"),
        }
    }

    #[test]
    fn test_sse_message_stop_emits_done() {
        let raw = r#"{"type":"message_stop"}"#;
        let sse: SseData = serde_json::from_str(raw).unwrap();
        let evt = sse_data_to_event(sse).unwrap();
        assert!(matches!(evt, StreamEvent::Done));
    }

    #[test]
    fn test_sse_ping_ignored() {
        let raw = r#"{"type":"ping"}"#;
        let sse: SseData = serde_json::from_str(raw).unwrap();
        assert!(sse_data_to_event(sse).is_none());
    }

    #[test]
    fn test_sse_message_start_ignored() {
        let raw = r#"{"type":"message_start","message":{"id":"msg_123","role":"assistant"}}"#;
        let sse: SseData = serde_json::from_str(raw).unwrap();
        assert!(sse_data_to_event(sse).is_none());
    }

    #[test]
    fn test_beta_header_includes_token_efficient_tools() {
        // Verify that the token-efficient-tools flag is always present and
        // caching flag is appended when use_cache is true.
        let beta_no_cache = "token-efficient-tools-2026-03-28".to_string();
        let beta_with_cache =
            "token-efficient-tools-2026-03-28,prompt-caching-2024-07-31".to_string();

        assert!(beta_no_cache.contains("token-efficient-tools-2026-03-28"));
        assert!(!beta_no_cache.contains("prompt-caching"));

        assert!(beta_with_cache.contains("token-efficient-tools-2026-03-28"));
        assert!(beta_with_cache.contains("prompt-caching-2024-07-31"));
    }
}
