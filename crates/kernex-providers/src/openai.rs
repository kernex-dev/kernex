//! OpenAI-compatible API provider.
//!
//! Works with OpenAI's API and any compatible endpoint.
//! Exports `pub(crate)` types and the agentic loop reused by the OpenRouter provider.

use crate::error::ProviderError;
use async_trait::async_trait;
use kernex_core::{
    context::{ApiMessage, Context},
    error::KernexError,
    message::Response,
    traits::Provider,
};
use secrecy::{ExposeSecret, SecretString};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::{read_truncated_error_body, send_with_retry};
use crate::openai_responses::{responses_agentic_complete, responses_complete};
use crate::tools::{build_response, tools_enabled, ToolExecutor};

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// Which OpenAI API a provider sends its requests to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiApi {
    /// `POST {base_url}/chat/completions`, which every OpenAI-compatible
    /// server implements.
    ChatCompletions,
    /// `POST {base_url}/responses`, OpenAI's Responses API. GPT-6.2 Sol and
    /// Astra, GPT-6.1 Sol and Terra, and GPT-6 Astra call tools only here.
    Responses,
}

impl OpenAiApi {
    /// The API for a base URL: Responses for OpenAI's own host
    /// (`api.openai.com` and its regional hosts), Chat Completions for any
    /// other, which may implement nothing else.
    pub fn for_base_url(base_url: &str) -> Self {
        if is_openai_host(base_url) {
            Self::Responses
        } else {
            Self::ChatCompletions
        }
    }
}

/// OpenAI-compatible provider with tool-execution loop.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api: OpenAiApi,
    api_key: SecretString,
    model: String,
    name: String,
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
    sandbox_dirs: crate::SandboxDirs,
    web_fetch: crate::WebFetchPolicy,
}

impl OpenAiProvider {
    /// Create from config values. The API follows the base URL (see
    /// [`OpenAiApi::for_base_url`]); [`OpenAiProvider::with_api`] overrides it.
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
                .map_err(|e| ProviderError::Logic(format!("failed to build HTTP client: {e}")))?,
            api: OpenAiApi::for_base_url(&base_url),
            base_url,
            api_key: SecretString::new(api_key),
            model,
            name: "openai".to_string(),
            workspace_path,
            sandbox_profile: Default::default(),
            sandbox_dirs: Default::default(),
            web_fetch: Default::default(),
        })
    }

    /// Override the HTTP request timeout. Defaults to 120 s.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(secs))
            .build()
        {
            self.client = client;
        }
        self
    }

    /// Override the provider name reported by [`Provider::name`].
    ///
    /// Used by the factory to give OpenAI-compatible variants (groq, mistral,
    /// deepseek, etc.) their own identity for logging and metrics.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Send requests to `api` whatever the base URL: Responses for a proxy
    /// or host that implements it, Chat Completions to keep OpenAI itself
    /// on the older API.
    pub fn with_api(mut self, api: OpenAiApi) -> Self {
        self.api = api;
        self
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }

    /// Set the runtime directories the sandbox protects from tool calls.
    pub fn with_sandbox_dirs(mut self, dirs: crate::SandboxDirs) -> Self {
        self.sandbox_dirs = dirs;
        self
    }

    /// Set which hosts the `web_fetch` tool may reach. The default is
    /// disabled: the tool is not offered to the model.
    pub fn with_web_fetch_policy(mut self, policy: crate::WebFetchPolicy) -> Self {
        self.web_fetch = policy;
        self
    }
}

pub(crate) use crate::openai_wire::{
    build_openai_messages, chat_tool_effort, is_openai_host, needs_responses_for_tools,
    to_openai_tools, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};

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
    token_budget: Option<u64>,
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
            reasoning_effort: chat_tool_effort(provider_name, model).filter(|_| tools.is_some()),
        };

        debug!("{provider_name}: POST {url} model={model} turn={turn}");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Logic(format!("{provider_name}: serialize failed: {e}")))?;

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
            let text = read_truncated_error_body(resp).await;
            return Err(
                ProviderError::Logic(format!("{provider_name} returned {status}: {text}")).into(),
            );
        }

        let parsed: ChatCompletionResponse = resp.json().await.map_err(|e| {
            ProviderError::Logic(format!("{provider_name}: failed to parse response: {e}"))
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

                // Stop before starting another turn once the billed spend has
                // reached the caller's budget. A final text answer below would
                // already have returned, so nothing completed is ever
                // discarded here.
                if kernex_core::run::budget_exhausted(total_tokens, token_budget) {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let mut resp = build_response(
                        String::new(),
                        provider_name,
                        total_tokens,
                        elapsed_ms,
                        last_model,
                    );
                    resp.metadata.stop_reason = Some("budget_exhausted".to_string());
                    return Ok(resp);
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
    // Turn budget exhausted without a final answer: empty text +
    // stop_reason="max_turns" so the runtime surfaces RunOutcome::MaxTurns
    // instead of a synthetic answer.
    let mut resp = build_response(
        String::new(),
        provider_name,
        total_tokens,
        elapsed_ms,
        last_model,
    );
    resp.metadata.stop_reason = Some("max_turns".to_string());
    Ok(resp)
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
        let base = self.base_url.trim_end_matches('/');
        let url = match self.api {
            OpenAiApi::ChatCompletions => format!("{base}/chat/completions"),
            OpenAiApi::Responses => format!("{base}/responses"),
        };
        let auth = format!("Bearer {}", self.api_key.expose_secret());
        let max_turns = context.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                if self.api == OpenAiApi::ChatCompletions
                    && is_openai_host(&self.base_url)
                    && needs_responses_for_tools(effective_model)
                {
                    return Err(ProviderError::Logic(format!(
                        "{name}: {effective_model} calls tools only through OpenAI's Responses API, and this provider is set to Chat Completions; use OpenAiApi::Responses, or call it without tools"
                    ))
                    .into());
                }
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone())
                    .with_sandbox_dirs(&self.sandbox_dirs)
                    .with_web_fetch_policy(self.web_fetch.clone())
                    .with_hook_runner_opt(context.hook_runner.clone())
                    .with_permission_rules_opt(context.permission_rules.clone())
                    .with_allowed_tools(context.allowed_tools.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let (client, budget) = (&self.client, context.token_budget);
                let (model, sys, msgs) = (effective_model, &system, &api_messages);
                let result = match self.api {
                    OpenAiApi::ChatCompletions => {
                        openai_agentic_complete(
                            client,
                            &url,
                            &auth,
                            model,
                            sys,
                            msgs,
                            &mut executor,
                            max_turns,
                            budget,
                            name,
                        )
                        .await
                    }
                    OpenAiApi::Responses => {
                        responses_agentic_complete(
                            client,
                            &url,
                            &auth,
                            model,
                            sys,
                            msgs,
                            &mut executor,
                            max_turns,
                            budget,
                            name,
                        )
                        .await
                    }
                };

                executor.shutdown_mcp().await;
                return result;
            }
        }

        // Fallback: no tools (classification calls, or no workspace).
        if self.api == OpenAiApi::Responses {
            return responses_complete(
                &self.client,
                &url,
                &auth,
                effective_model,
                &system,
                &api_messages,
                name,
            )
            .await;
        }
        let start = Instant::now();
        let messages = build_openai_messages(&system, &api_messages);
        let body = ChatCompletionRequest {
            model: effective_model.to_string(),
            messages,
            tools: None,
            reasoning_effort: None,
        };

        debug!("{name}: POST {url} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Logic(format!("{name}: serialize failed: {e}")))?;

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
            let text = read_truncated_error_body(resp).await;
            return Err(ProviderError::Logic(format!("{name} returned {status}: {text}")).into());
        }

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Logic(format!("{name}: failed to parse response: {e}")))?;

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
            reasoning_effort: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("tools").is_none());
        assert!(json.get("reasoning_effort").is_none());
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
            reasoning_effort: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("tools").unwrap().as_array().unwrap().len() == 7);
    }

    /// A stub for both APIs: it answers each request with a final text
    /// message ("done") in the shape of the API the request went to, and
    /// records each request body with the request target under `__target`.
    async fn spawn_chat_stub() -> (
        std::net::SocketAddr,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let bodies_srv = bodies.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let bodies = bodies_srv.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 262_144];
                    let mut read = 0usize;
                    let mut body_start = 0usize;
                    loop {
                        let n = sock.read(&mut buf[read..]).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        read += n;
                        let s = String::from_utf8_lossy(&buf[..read]);
                        if let Some(h_end) = s.find("\r\n\r\n") {
                            body_start = h_end + 4;
                            let cl = s.lines().find_map(|l| {
                                let low = l.to_ascii_lowercase();
                                low.strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            });
                            if read - body_start >= cl.unwrap_or(0) {
                                break;
                            }
                        }
                        if read == buf.len() {
                            break;
                        }
                    }
                    // The request line's target: a path, or an absolute URL
                    // when the client sends through a proxy.
                    let head = String::from_utf8_lossy(&buf[..body_start]).to_string();
                    let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    if let Ok(mut v) =
                        serde_json::from_slice::<serde_json::Value>(&buf[body_start..read])
                    {
                        v["__target"] = target.clone().into();
                        bodies.lock().unwrap().push(v);
                    }
                    let body = if target.ends_with("/responses") {
                        r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}],"model":"stub","usage":{"total_tokens":3}}"#
                    } else {
                        r#"{"choices":[{"message":{"role":"assistant","content":"done"}}],"model":"stub","usage":{"total_tokens":3}}"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (addr, bodies)
    }

    /// Run one request through `provider` and return the single body sent.
    async fn sent_body(
        provider: OpenAiProvider,
        bodies: &std::sync::Mutex<Vec<serde_json::Value>>,
    ) -> serde_json::Value {
        let resp = provider.complete(&Context::new("hi")).await.unwrap();
        assert_eq!(resp.text, "done");
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        bodies[0].clone()
    }

    #[tokio::test]
    async fn luna_with_tools_is_sent_reasoning_effort_none() {
        let ws = tempfile::tempdir().unwrap();
        let (addr, bodies) = spawn_chat_stub().await;
        let p = OpenAiProvider::from_config(
            format!("http://{addr}/v1"),
            "k".into(),
            "gpt-6-luna".into(),
            Some(ws.path().to_path_buf()),
        )
        .unwrap();
        let body = sent_body(p, &bodies).await;
        assert!(body["tools"].as_array().is_some_and(|t| !t.is_empty()));
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[tokio::test]
    async fn luna_without_tools_gets_no_reasoning_effort() {
        let (addr, bodies) = spawn_chat_stub().await;
        let p = OpenAiProvider::from_config(
            format!("http://{addr}/v1"),
            "k".into(),
            "gpt-6-luna".into(),
            None,
        )
        .unwrap();
        let body = sent_body(p, &bodies).await;
        assert!(body.get("tools").is_none());
        assert!(body.get("reasoning_effort").is_none(), "{body}");
    }

    #[tokio::test]
    async fn compatible_providers_get_no_reasoning_effort() {
        let ws = tempfile::tempdir().unwrap();
        let (addr, bodies) = spawn_chat_stub().await;
        let p = OpenAiProvider::from_config(
            format!("http://{addr}/v1"),
            "k".into(),
            "gpt-6-luna".into(),
            Some(ws.path().to_path_buf()),
        )
        .unwrap()
        .with_name("groq");
        let body = sent_body(p, &bodies).await;
        assert!(body["tools"].as_array().is_some_and(|t| !t.is_empty()));
        assert!(body.get("reasoning_effort").is_none(), "{body}");

        // OpenRouter runs the same loop under its own name.
        let (addr, bodies) = spawn_chat_stub().await;
        let mut executor = ToolExecutor::new(ws.path().to_path_buf());
        let resp = openai_agentic_complete(
            &reqwest::Client::new(),
            &format!("http://{addr}/v1/chat/completions"),
            "Bearer k",
            "gpt-6-luna",
            "",
            &[ApiMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
            &mut executor,
            2,
            None,
            "openrouter",
        )
        .await
        .unwrap();
        assert_eq!(resp.text, "done");
        let bodies = bodies.lock().unwrap();
        assert!(bodies[0]["tools"].as_array().is_some_and(|t| !t.is_empty()));
        assert!(bodies[0].get("reasoning_effort").is_none(), "{}", bodies[0]);
    }

    /// A provider for OpenAI's own host whose HTTP client sends every
    /// request to the local stub at `addr` instead, so a regression cannot
    /// reach the real API.
    fn openai_host_via_stub(
        addr: std::net::SocketAddr,
        model: &str,
        ws: Option<PathBuf>,
    ) -> OpenAiProvider {
        let mut p = OpenAiProvider::from_config(
            "http://api.openai.com/v1".into(),
            "k".into(),
            model.into(),
            ws,
        )
        .unwrap();
        p.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::http(format!("http://{addr}")).unwrap())
            .build()
            .unwrap();
        p
    }

    #[test]
    fn the_api_follows_the_host() {
        for url in ["https://api.openai.com/v1", "https://eu.api.openai.com/v1"] {
            assert_eq!(OpenAiApi::for_base_url(url), OpenAiApi::Responses, "{url}");
        }
        for url in [
            "http://localhost:4000/v1",
            "https://api.groq.com/openai/v1",
            "https://api.openai.com.evil.test/v1",
        ] {
            assert_eq!(
                OpenAiApi::for_base_url(url),
                OpenAiApi::ChatCompletions,
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn openai_itself_gets_the_responses_api_with_and_without_tools() {
        let ws = tempfile::tempdir().unwrap();
        let (addr, bodies) = spawn_chat_stub().await;
        let p = openai_host_via_stub(addr, "gpt-6.2-sol", Some(ws.path().to_path_buf()));
        let body = sent_body(p, &bodies).await;
        assert_eq!(body["__target"], "http://api.openai.com/v1/responses");
        assert_eq!(body["model"], "gpt-6.2-sol");
        assert_eq!(body["store"], false);
        let tools = body["tools"].as_array().unwrap();
        assert!(!tools.is_empty() && tools.iter().all(|t| t.get("function").is_none()));
        assert!(body.get("messages").is_none() && body.get("reasoning_effort").is_none());

        let (addr, bodies) = spawn_chat_stub().await;
        let p = openai_host_via_stub(addr, "gpt-6-luna", None);
        let body = sent_body(p, &bodies).await;
        assert_eq!(body["__target"], "http://api.openai.com/v1/responses");
        assert!(body.get("tools").is_none(), "{body}");
        assert_eq!(body["input"][0]["content"], "hi");
    }

    #[tokio::test]
    async fn chat_completions_on_openai_itself_still_refuses_responses_only_tools() {
        let ws = tempfile::tempdir().unwrap();
        let (addr, bodies) = spawn_chat_stub().await;
        let p = openai_host_via_stub(addr, "gpt-6.2-sol", Some(ws.path().to_path_buf()))
            .with_api(OpenAiApi::ChatCompletions);
        let err = p
            .complete(&Context::new("hi"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("gpt-6.2-sol"), "{err}");
        assert!(err.contains("Responses API"), "{err}");
        assert!(bodies.lock().unwrap().is_empty(), "a request was sent");

        // Without tools the call goes through, to Chat Completions.
        let (addr, bodies) = spawn_chat_stub().await;
        let p =
            openai_host_via_stub(addr, "gpt-6.2-sol", None).with_api(OpenAiApi::ChatCompletions);
        let body = sent_body(p, &bodies).await;
        assert_eq!(
            body["__target"],
            "http://api.openai.com/v1/chat/completions"
        );
        assert!(body.get("tools").is_none());
        assert!(body.get("reasoning_effort").is_none(), "{body}");
    }

    #[tokio::test]
    async fn another_host_keeps_chat_completions_unless_told_otherwise() {
        // A proxy may translate the call itself, so it gets no refusal.
        let ws = tempfile::tempdir().unwrap();
        let proxy = |addr: std::net::SocketAddr| {
            OpenAiProvider::from_config(
                format!("http://{addr}/v1"),
                "k".into(),
                "gpt-6.2-sol".into(),
                Some(ws.path().to_path_buf()),
            )
            .unwrap()
        };
        let (addr, bodies) = spawn_chat_stub().await;
        let body = sent_body(proxy(addr), &bodies).await;
        assert_eq!(body["__target"], "/v1/chat/completions");
        assert!(body["tools"].as_array().is_some_and(|t| !t.is_empty()));
        assert!(body.get("reasoning_effort").is_none(), "{body}");

        let (addr, bodies) = spawn_chat_stub().await;
        let body = sent_body(proxy(addr).with_api(OpenAiApi::Responses), &bodies).await;
        assert_eq!(body["__target"], "/v1/responses");
        assert_eq!(body["store"], false);
    }
}
