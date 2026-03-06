//! Anthropic API provider with tool-execution loop.
//!
//! Calls the Anthropic Messages API directly (not via Claude Code CLI).
//! Uses content blocks (text/tool_use/tool_result) for tool calling.

use async_trait::async_trait;
use kernex_core::{context::Context, error::KernexError, message::Response, traits::Provider};
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

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    workspace_path: Option<PathBuf>,
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
            api_key,
            model,
            max_tokens,
            workspace_path,
        })
    }
}

// --- Serde types for the Anthropic Messages API ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    system: String,
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

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;

                let result = self
                    .agentic_loop(
                        effective_model,
                        &system,
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
            system,
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
                let req = client
                    .post(ANTHROPIC_API_URL)
                    .header("x-api-key", api_key.as_str())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .header("content-type", "application/json")
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
        if self.api_key.is_empty() {
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
        system: &str,
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
                system: system.to_string(),
                messages: messages.clone(),
                tools: tools.clone(),
            };

            debug!("anthropic: POST {ANTHROPIC_API_URL} model={model} turn={turn}");

            let body_json = serde_json::to_vec(&body)
                .map_err(|e| KernexError::Provider(format!("anthropic: serialize failed: {e}")))?;

            let resp = {
                let client = &self.client;
                let api_key = &self.api_key;
                send_with_retry("anthropic", || {
                    let req = client
                        .post(ANTHROPIC_API_URL)
                        .header("x-api-key", api_key.as_str())
                        .header("anthropic-version", ANTHROPIC_VERSION)
                        .header("content-type", "application/json")
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
            system: "Be helpful.".into(),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("Hello".into()),
            }],
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["max_tokens"], 8192);
        assert_eq!(json["system"], "Be helpful.");
        assert_eq!(json["messages"][0]["role"], "user");
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_anthropic_request_empty_system_omitted() {
        let body = AnthropicRequest {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 8192,
            system: String::new(),
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
            system: "test".into(),
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
}
