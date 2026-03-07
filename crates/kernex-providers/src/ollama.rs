//! Ollama local model provider with tool-execution loop.
//!
//! Connects to a locally running Ollama server. No API key required.
//! Tool calling format is similar to OpenAI but has no `tool_call_id`.

use async_trait::async_trait;
use kernex_core::{context::Context, error::KernexError, message::Response, traits::Provider};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::send_with_retry;
use crate::tools::{build_response, tools_enabled, ToolDef, ToolExecutor};

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// Ollama provider backed by a local server.
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl OllamaProvider {
    /// Create from config values.
    pub fn from_config(
        base_url: String,
        model: String,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| KernexError::Provider(format!("failed to build HTTP client: {e}")))?,
            base_url,
            model,
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

// --- Serde types ---

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaToolDef>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct OllamaChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct OllamaFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct OllamaToolDef {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaFunctionDef,
}

#[derive(Serialize, Clone)]
struct OllamaFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
}

/// Convert ToolDef to Ollama format.
fn to_ollama_tools(defs: &[ToolDef]) -> Vec<OllamaToolDef> {
    defs.iter()
        .map(|d| OllamaToolDef {
            tool_type: "function".to_string(),
            function: OllamaFunctionDef {
                name: d.name.clone(),
                description: d.description.clone(),
                parameters: d.parameters.clone(),
            },
        })
        .collect()
}

/// Build Ollama-format messages from context.
fn build_ollama_messages(
    system: &str,
    api_messages: &[kernex_core::context::ApiMessage],
) -> Vec<OllamaChatMessage> {
    let mut messages = Vec::with_capacity(api_messages.len() + 1);
    if !system.is_empty() {
        messages.push(OllamaChatMessage {
            role: "system".to_string(),
            content: Some(system.to_string()),
            tool_calls: None,
        });
    }
    for m in api_messages {
        messages.push(OllamaChatMessage {
            role: m.role.clone(),
            content: Some(m.content.clone()),
            tool_calls: None,
        });
    }
    messages
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model);
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let max_turns = context.max_turns.unwrap_or(DEFAULT_MAX_TURNS);
        let start = Instant::now();

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let result = self
                    .agentic_loop(
                        &url,
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
        let mut messages = build_ollama_messages(&system, &api_messages);
        let simple_msgs: Vec<_> = messages
            .drain(..)
            .map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content.unwrap_or_default()
                })
            })
            .collect();

        let body = serde_json::json!({
            "model": effective_model,
            "messages": simple_msgs,
            "stream": false
        });

        debug!("ollama: POST {url} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("ollama: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let url = &url;
            send_with_retry("ollama", || {
                let req = client
                    .post(url.as_str())
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
                "ollama returned {status}: {text}"
            )));
        }

        let parsed: OllamaChatResponse = resp
            .json()
            .await
            .map_err(|e| KernexError::Provider(format!("ollama: failed to parse response: {e}")))?;

        let text = parsed
            .message
            .and_then(|m| m.content)
            .unwrap_or_else(|| "No response from Ollama.".to_string());

        let tokens = match (parsed.eval_count, parsed.prompt_eval_count) {
            (Some(e), Some(p)) => e + p,
            (Some(e), None) => e,
            _ => 0,
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(
            text,
            "ollama",
            tokens,
            elapsed_ms,
            parsed.model,
        ))
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("ollama not available: {e}");
                false
            }
        }
    }
}

impl OllamaProvider {
    /// Ollama-specific agentic loop.
    async fn agentic_loop(
        &self,
        url: &str,
        model: &str,
        system: &str,
        api_messages: &[kernex_core::context::ApiMessage],
        executor: &mut ToolExecutor,
        max_turns: u32,
    ) -> Result<Response, KernexError> {
        let start = Instant::now();

        let mut messages = build_ollama_messages(system, api_messages);
        let all_tool_defs = executor.all_tool_defs();
        let tools = if all_tool_defs.is_empty() {
            None
        } else {
            Some(to_ollama_tools(&all_tool_defs))
        };

        let mut last_model: Option<String> = None;
        let mut total_tokens: u64 = 0;

        for turn in 0..max_turns {
            let body = OllamaChatRequest {
                model: model.to_string(),
                messages: messages.clone(),
                stream: false,
                tools: tools.clone(),
            };

            debug!("ollama: POST {url} model={model} turn={turn}");

            let body_json = serde_json::to_vec(&body)
                .map_err(|e| KernexError::Provider(format!("ollama: serialize failed: {e}")))?;

            let resp = {
                let client = &self.client;
                send_with_retry("ollama", || {
                    let req = client
                        .post(url)
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
                    "ollama returned {status}: {text}"
                )));
            }

            let parsed: OllamaChatResponse = resp.json().await.map_err(|e| {
                KernexError::Provider(format!("ollama: failed to parse response: {e}"))
            })?;

            if let Some(ref m) = parsed.model {
                last_model = Some(m.clone());
            }
            if let Some(e) = parsed.eval_count {
                total_tokens += e;
            }
            if let Some(p) = parsed.prompt_eval_count {
                total_tokens += p;
            }

            let Some(assistant_msg) = parsed.message else {
                break;
            };

            // Check for tool calls.
            if let Some(ref tool_calls) = assistant_msg.tool_calls {
                if !tool_calls.is_empty() {
                    messages.push(assistant_msg.clone());

                    for tc in tool_calls {
                        info!("ollama: tool call [{turn}] {}", tc.function.name);

                        let result = executor
                            .execute(&tc.function.name, &tc.function.arguments)
                            .await;

                        // Ollama uses role "tool" for tool results.
                        messages.push(OllamaChatMessage {
                            role: "tool".to_string(),
                            content: Some(result.content),
                            tool_calls: None,
                        });
                    }

                    continue;
                }
            }

            // Text-only response.
            let text = assistant_msg
                .content
                .unwrap_or_else(|| "No response from Ollama.".to_string());

            let elapsed_ms = start.elapsed().as_millis() as u64;
            return Ok(build_response(
                text,
                "ollama",
                total_tokens,
                elapsed_ms,
                last_model,
            ));
        }

        // Max turns exhausted.
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(build_response(
            format!("ollama: reached max turns ({max_turns}) without final response"),
            "ollama",
            total_tokens,
            elapsed_ms,
            last_model,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_provider_name() {
        let p = OllamaProvider::from_config("http://localhost:11434".into(), "llama3".into(), None)
            .unwrap();
        assert_eq!(p.name(), "ollama");
        assert!(!p.requires_api_key());
    }

    #[test]
    fn test_ollama_request_serialization() {
        let body = OllamaChatRequest {
            model: "llama3".into(),
            messages: vec![
                OllamaChatMessage {
                    role: "system".into(),
                    content: Some("Be helpful.".into()),
                    tool_calls: None,
                },
                OllamaChatMessage {
                    role: "user".into(),
                    content: Some("Hello".into()),
                    tool_calls: None,
                },
            ],
            stream: false,
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "llama3");
        assert!(!json["stream"].as_bool().unwrap());
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_ollama_request_with_tools() {
        let defs = crate::tools::builtin_tool_defs();
        let tools = to_ollama_tools(&defs);
        let body = OllamaChatRequest {
            model: "llama3".into(),
            messages: vec![OllamaChatMessage {
                role: "user".into(),
                content: Some("list files".into()),
                tool_calls: None,
            }],
            stream: false,
            tools: Some(tools),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tools"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn test_ollama_response_parsing() {
        let json = r#"{"message":{"role":"assistant","content":"Hi there!"},"model":"llama3","eval_count":42,"prompt_eval_count":10}"#;
        let resp: OllamaChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.message.as_ref().unwrap().content.as_deref(),
            Some("Hi there!")
        );
        assert_eq!(resp.model, Some("llama3".into()));
        assert_eq!(resp.eval_count, Some(42));
    }

    #[test]
    fn test_ollama_tool_call_response_parsing() {
        let json = r#"{"message":{"role":"assistant","tool_calls":[{"function":{"name":"bash","arguments":{"command":"ls"}}}]},"model":"llama3"}"#;
        let resp: OllamaChatResponse = serde_json::from_str(json).unwrap();
        let msg = resp.message.unwrap();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "bash");
        assert_eq!(tcs[0].function.arguments["command"], "ls");
    }
}
