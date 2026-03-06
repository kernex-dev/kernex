//! Google Gemini API provider with tool-execution loop.
//!
//! Calls the Gemini `generateContent` endpoint. Auth via `x-goog-api-key` header.
//! Uses `functionCall` / `functionResponse` parts for tool calling.

use async_trait::async_trait;
use kernex_core::{context::Context, error::KernexError, message::Response, traits::Provider};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::send_with_retry;
use crate::tools::{build_response, tools_enabled, ToolDef, ToolExecutor};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// Google Gemini API provider.
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    workspace_path: Option<PathBuf>,
}

impl GeminiProvider {
    /// Create from config values.
    pub fn from_config(
        api_key: String,
        model: String,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| KernexError::Provider(format!("failed to build HTTP client: {e}")))?,
            api_key,
            model,
            workspace_path,
        })
    }
}

// --- Serde types ---

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolDeclaration>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiToolDeclaration {
    function_declarations: Vec<GeminiFunctionDef>,
}

#[derive(Serialize, Clone)]
struct GeminiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    #[serde(default)]
    total_token_count: u64,
}

/// Convert ToolDef to Gemini format.
fn to_gemini_tools(defs: &[ToolDef]) -> Vec<GeminiToolDeclaration> {
    vec![GeminiToolDeclaration {
        function_declarations: defs
            .iter()
            .map(|d| GeminiFunctionDef {
                name: d.name.clone(),
                description: d.description.clone(),
                parameters: d.parameters.clone(),
            })
            .collect(),
    }]
}

/// Build Gemini contents from API messages.
fn build_gemini_contents(api_messages: &[kernex_core::context::ApiMessage]) -> Vec<GeminiContent> {
    api_messages
        .iter()
        .map(|m| {
            let role = if m.role == "assistant" {
                "model"
            } else {
                "user"
            };
            GeminiContent {
                role: Some(role.to_string()),
                parts: vec![GeminiPart {
                    text: Some(m.content.clone()),
                    function_call: None,
                    function_response: None,
                }],
            }
        })
        .collect()
}

#[async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
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
                executor.register_toolboxes(&context.toolboxes);

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
        let system_instruction = if system.is_empty() {
            None
        } else {
            Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart {
                    text: Some(system),
                    function_call: None,
                    function_response: None,
                }],
            })
        };

        let contents = build_gemini_contents(&api_messages);

        let body = GeminiRequest {
            contents,
            system_instruction,
            tools: None,
        };

        let url = format!("{GEMINI_BASE_URL}/models/{effective_model}:generateContent");
        debug!("gemini: POST models/{effective_model}:generateContent (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("gemini: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let url = &url;
            let api_key = &self.api_key;
            send_with_retry("gemini", || {
                let req = client
                    .post(url.as_str())
                    .header("x-goog-api-key", api_key.as_str())
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
                "gemini returned {status}: {text}"
            )));
        }

        let parsed: GeminiResponse = resp
            .json()
            .await
            .map_err(|e| KernexError::Provider(format!("gemini: failed to parse response: {e}")))?;

        let text = extract_text_from_response(&parsed);
        let tokens = parsed
            .usage_metadata
            .as_ref()
            .map(|u| u.total_token_count)
            .unwrap_or(0);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(
            text,
            "gemini",
            tokens,
            elapsed_ms,
            Some(effective_model.to_string()),
        ))
    }

    async fn is_available(&self) -> bool {
        if self.api_key.is_empty() {
            warn!("gemini: no API key configured");
            return false;
        }
        let url = format!("{GEMINI_BASE_URL}/models");
        match self
            .client
            .get(&url)
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("gemini not available: {e}");
                false
            }
        }
    }
}

impl GeminiProvider {
    /// Gemini-specific agentic loop using functionCall/functionResponse.
    async fn agentic_loop(
        &self,
        model: &str,
        system: &str,
        api_messages: &[kernex_core::context::ApiMessage],
        executor: &mut ToolExecutor,
        max_turns: u32,
    ) -> Result<Response, KernexError> {
        let start = Instant::now();

        let mut contents = build_gemini_contents(api_messages);

        let system_instruction = if system.is_empty() {
            None
        } else {
            Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart {
                    text: Some(system.to_string()),
                    function_call: None,
                    function_response: None,
                }],
            })
        };

        let all_tool_defs = executor.all_tool_defs();
        let tools = if all_tool_defs.is_empty() {
            None
        } else {
            Some(to_gemini_tools(&all_tool_defs))
        };

        let mut total_tokens: u64 = 0;

        for turn in 0..max_turns {
            let body = GeminiRequest {
                contents: contents.clone(),
                system_instruction: system_instruction.clone(),
                tools: tools.clone(),
            };

            let url = format!("{GEMINI_BASE_URL}/models/{model}:generateContent");
            debug!("gemini: POST models/{model}:generateContent turn={turn}");

            let body_json = serde_json::to_vec(&body)
                .map_err(|e| KernexError::Provider(format!("gemini: serialize failed: {e}")))?;

            let resp = {
                let client = &self.client;
                let url = &url;
                let api_key = &self.api_key;
                send_with_retry("gemini", || {
                    let req = client
                        .post(url.as_str())
                        .header("x-goog-api-key", api_key.as_str())
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
                    "gemini returned {status}: {text}"
                )));
            }

            let parsed: GeminiResponse = resp.json().await.map_err(|e| {
                KernexError::Provider(format!("gemini: failed to parse response: {e}"))
            })?;

            if let Some(ref u) = parsed.usage_metadata {
                total_tokens += u.total_token_count;
            }

            let candidate_content = parsed
                .candidates
                .as_ref()
                .and_then(|c| c.first())
                .and_then(|c| c.content.clone());

            let Some(response_content) = candidate_content else {
                break;
            };

            // Check for function calls in parts.
            let function_calls: Vec<&GeminiFunctionCall> = response_content
                .parts
                .iter()
                .filter_map(|p| p.function_call.as_ref())
                .collect();

            if !function_calls.is_empty() {
                // Append model response with function calls.
                contents.push(response_content.clone());

                // Build function responses.
                let mut response_parts = Vec::new();
                for fc in &function_calls {
                    info!("gemini: tool call [{turn}] {}", fc.name);

                    let result = executor.execute(&fc.name, &fc.args).await;

                    response_parts.push(GeminiPart {
                        text: None,
                        function_call: None,
                        function_response: Some(GeminiFunctionResponse {
                            name: fc.name.clone(),
                            response: serde_json::json!({
                                "result": result.content,
                                "is_error": result.is_error
                            }),
                        }),
                    });
                }

                contents.push(GeminiContent {
                    role: Some("user".to_string()),
                    parts: response_parts,
                });

                continue;
            }

            // Text-only response.
            let text = response_content
                .parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n");

            let text = if text.is_empty() {
                "No response from Gemini.".to_string()
            } else {
                text
            };

            let elapsed_ms = start.elapsed().as_millis() as u64;
            return Ok(build_response(
                text,
                "gemini",
                total_tokens,
                elapsed_ms,
                Some(model.to_string()),
            ));
        }

        // Max turns exhausted.
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(build_response(
            format!("gemini: reached max turns ({max_turns}) without final response"),
            "gemini",
            total_tokens,
            elapsed_ms,
            Some(model.to_string()),
        ))
    }
}

/// Extract text from a Gemini response.
fn extract_text_from_response(resp: &GeminiResponse) -> String {
    resp.candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| {
            c.parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "No response from Gemini.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_provider_name() {
        let p = GeminiProvider::from_config("AIza-test".into(), "gemini-2.0-flash".into(), None)
            .unwrap();
        assert_eq!(p.name(), "gemini");
        assert!(p.requires_api_key());
    }

    #[test]
    fn test_gemini_request_serialization() {
        let body = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart {
                    text: Some("Hello".into()),
                    function_call: None,
                    function_response: None,
                }],
            }],
            system_instruction: Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart {
                    text: Some("Be helpful.".into()),
                    function_call: None,
                    function_response: None,
                }],
            }),
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("systemInstruction").is_some());
        assert_eq!(json["contents"][0]["role"], "user");
        assert_eq!(json["contents"][0]["parts"][0]["text"], "Hello");
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn test_gemini_request_no_system() {
        let body = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart {
                    text: Some("Hello".into()),
                    function_call: None,
                    function_response: None,
                }],
            }],
            system_instruction: None,
            tools: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("systemInstruction").is_none());
    }

    #[test]
    fn test_gemini_request_with_tools() {
        let defs = crate::tools::builtin_tool_defs();
        let tools = to_gemini_tools(&defs);
        let body = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart {
                    text: Some("list files".into()),
                    function_call: None,
                    function_response: None,
                }],
            }],
            system_instruction: None,
            tools: Some(tools),
        };
        let json = serde_json::to_value(&body).unwrap();
        let decls = &json["tools"][0]["functionDeclarations"];
        assert_eq!(decls.as_array().unwrap().len(), 4);
    }

    #[test]
    fn test_gemini_role_mapping() {
        let api_msgs = vec![
            kernex_core::context::ApiMessage {
                role: "user".into(),
                content: "Hi".into(),
            },
            kernex_core::context::ApiMessage {
                role: "assistant".into(),
                content: "Hello!".into(),
            },
        ];
        let contents = build_gemini_contents(&api_msgs);
        assert_eq!(contents[0].role, Some("user".into()));
        assert_eq!(contents[1].role, Some("model".into()));
    }

    #[test]
    fn test_gemini_response_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hi there!"}]}}],"usageMetadata":{"totalTokenCount":25}}"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let text = extract_text_from_response(&resp);
        assert_eq!(text, "Hi there!");
        assert_eq!(
            resp.usage_metadata.as_ref().map(|u| u.total_token_count),
            Some(25)
        );
    }

    #[test]
    fn test_gemini_function_call_response_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"bash","args":{"command":"ls"}}}]}}],"usageMetadata":{"totalTokenCount":30}}"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let content = resp
            .candidates
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .content
            .unwrap();
        let fc = content.parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "bash");
        assert_eq!(fc.args["command"], "ls");
    }

    #[test]
    fn test_gemini_function_response_serialization() {
        let part = GeminiPart {
            text: None,
            function_call: None,
            function_response: Some(GeminiFunctionResponse {
                name: "bash".into(),
                response: serde_json::json!({"result": "file1.txt\nfile2.txt", "is_error": false}),
            }),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert!(json.get("text").is_none());
        assert!(json.get("functionCall").is_none());
        assert_eq!(json["functionResponse"]["name"], "bash");
    }
}
