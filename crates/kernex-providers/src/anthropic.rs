//! Anthropic API provider with tool-execution loop.
//!
//! Calls the Anthropic Messages API directly (not via Claude Code CLI).
//! Uses content blocks (text/tool_use/tool_result) for tool calling.

use crate::error::ProviderError;
use async_trait::async_trait;
use kernex_core::{
    context::Context,
    error::KernexError,
    message::Response,
    stream::{StreamEvent, StreamUsage},
    traits::{Provider, StreamingProvider},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::http_retry::{read_truncated_error_body, send_with_retry};
use crate::tools::{
    build_response_with_usage, tools_enabled, ToolDef, ToolExecutor, UsageBreakdown,
};

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
                .map_err(|e| ProviderError::Logic(format!("failed to build HTTP client: {e}")))?,
            api_key: SecretString::new(api_key),
            model,
            max_tokens,
            workspace_path,
            sandbox_profile: Default::default(),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

/// The `thinking` request parameter. Adaptive thinking is the GA mechanism on
/// the Claude 4.6+ family: the model decides when and how much to think, and it
/// enables interleaved thinking (reasoning between tool calls) automatically
/// with no beta header. It is sent only when the caller opts in via
/// `Context::extended_thinking`; an absent field means thinking is off, which
/// is the API default. Manual extended thinking (`type: "enabled"` +
/// `budget_tokens`) is deprecated on 4.6 and rejected on 4.7+, so it is not
/// offered here.
#[derive(Serialize, Clone, Copy)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ThinkingConfig {
    Adaptive,
}

/// Build the `thinking` request field from the caller's opt-in flag.
fn thinking_config(extended_thinking: bool) -> Option<ThinkingConfig> {
    extended_thinking.then_some(ThinkingConfig::Adaptive)
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
    /// A thinking block replayed back to the API. When thinking is enabled and
    /// the model emits a tool_use, the thinking block(s) that preceded it must
    /// be sent back verbatim (signature included) on the follow-up request, or
    /// the API rejects it.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An encrypted thinking block, replayed verbatim like `Thinking`.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
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
    /// A thinking block. Present whenever thinking is enabled; the `thinking`
    /// field is empty unless `display: "summarized"` was requested, but the
    /// block (and its `signature`) must still be preserved for replay. Without
    /// this variant a real thinking block would fail the whole response parse.
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    /// An encrypted thinking block (safety-redacted reasoning).
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Map a response content block to the request block that replays the assistant
/// turn back to the API. The full content (thinking, text, and tool_use blocks)
/// must be replayed in order after a tool_use turn; thinking blocks carry a
/// signature that has to survive the round trip unchanged.
fn assistant_block_from_response(block: &AnthropicResponseBlock) -> AnthropicContentBlock {
    match block {
        AnthropicResponseBlock::Text { text } => AnthropicContentBlock::Text { text: text.clone() },
        AnthropicResponseBlock::Thinking {
            thinking,
            signature,
        } => AnthropicContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        },
        AnthropicResponseBlock::RedactedThinking { data } => {
            AnthropicContentBlock::RedactedThinking { data: data.clone() }
        }
        AnthropicResponseBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
    }
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    /// Tokens served from the prompt cache. Anthropic only emits this when
    /// `cache_control` blocks were present on the request and the cache
    /// matched; otherwise the field is absent.
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    /// Tokens written into the prompt cache. Present when a cache miss
    /// triggered a fresh write, absent on pure read-or-no-cache responses.
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
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

/// Reject a request whose message sequence ends with an assistant turn.
///
/// The Claude 4.6+ family no longer supports assistant prefill (a trailing
/// assistant message), and such a request 400s at the API. `Context::
/// to_api_messages` always appends the current user turn, so this should never
/// fire in practice; it is a defensive guard that turns a would-be opaque 400
/// into a clear, actionable error if a future code path or a direct message
/// construction ever breaks the user-turn-last invariant.
fn ensure_no_assistant_prefill(messages: &[AnthropicMessage]) -> Result<(), KernexError> {
    if messages.last().is_some_and(|m| m.role == "assistant") {
        return Err(ProviderError::Logic(
            "anthropic: message sequence ends with an assistant turn; the 4.6+ \
             model family does not support assistant prefill, so the final \
             message must be a user turn"
                .to_string(),
        )
        .into());
    }
    Ok(())
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

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone())
                    .with_hook_runner_opt(context.hook_runner.clone())
                    .with_permission_rules_opt(context.permission_rules.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let result = self
                    .agentic_loop(
                        effective_model,
                        &system_blocks,
                        context.extended_thinking,
                        &api_messages,
                        &mut executor,
                        max_turns,
                        context.token_budget,
                    )
                    .await;

                executor.shutdown_mcp().await;
                return result;
            }
        }

        // Fallback: no tools.
        let extended_thinking = context.extended_thinking;
        let start = Instant::now();
        let messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();
        ensure_no_assistant_prefill(&messages)?;

        let body = AnthropicRequest {
            model: effective_model.to_string(),
            max_tokens: self.max_tokens,
            system: system_blocks,
            messages,
            tools: None,
            thinking: thinking_config(extended_thinking),
        };

        debug!("anthropic: POST {ANTHROPIC_API_URL} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Logic(format!("anthropic: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let api_key = &self.api_key;
            send_with_retry("anthropic", || {
                // No anthropic-beta header: adaptive thinking rides in the
                // request body, and prompt caching is GA (cache_control on the
                // system blocks needs no beta header).
                let req = client
                    .post(ANTHROPIC_API_URL)
                    .header("x-api-key", api_key.expose_secret().as_str())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .header("content-type", "application/json")
                    .body(body_json.clone());
                async move { req.send().await }
            })
            .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = read_truncated_error_body(resp).await;
            return Err(
                ProviderError::Logic(format!("anthropic returned {status}: {text}")).into(),
            );
        }

        let parsed: AnthropicResponse = resp.json().await.map_err(|e| {
            ProviderError::Logic(format!("anthropic: failed to parse response: {e}"))
        })?;

        let text = extract_text_from_response(&parsed);
        // Sum every billed dimension so the historical `tokens_used` total
        // stays correct even when cache reads/writes are present.
        let tokens = parsed
            .usage
            .as_ref()
            .map(|u| {
                u.input_tokens
                    + u.output_tokens
                    + u.cache_read_input_tokens.unwrap_or(0)
                    + u.cache_creation_input_tokens.unwrap_or(0)
            })
            .unwrap_or(0);
        let usage = parsed
            .usage
            .as_ref()
            .map(|u| UsageBreakdown {
                input_tokens: Some(u.input_tokens),
                output_tokens: Some(u.output_tokens),
                cache_read_tokens: u.cache_read_input_tokens,
                cache_creation_tokens: u.cache_creation_input_tokens,
            })
            .unwrap_or_default();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Surface the upstream stop reason (end_turn / max_tokens / refusal /
        // ...) on the no-tools path too, so a truncated reply is detectable
        // here exactly as it is in the agentic loop.
        let stop_reason = parsed.stop_reason.clone();
        let mut resp =
            build_response_with_usage(text, "anthropic", tokens, elapsed_ms, parsed.model, usage);
        resp.metadata.stop_reason = stop_reason;
        Ok(resp)
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
    #[allow(clippy::too_many_arguments)]
    async fn agentic_loop(
        &self,
        model: &str,
        system_blocks: &[SystemBlock],
        extended_thinking: bool,
        api_messages: &[kernex_core::context::ApiMessage],
        executor: &mut ToolExecutor,
        max_turns: u32,
        token_budget: Option<u64>,
    ) -> Result<Response, KernexError> {
        let start = Instant::now();

        let mut messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();
        ensure_no_assistant_prefill(&messages)?;

        let all_tool_defs = executor.all_tool_defs();
        let tools = if all_tool_defs.is_empty() {
            None
        } else {
            Some(to_anthropic_tools(&all_tool_defs))
        };

        let mut last_model: Option<String> = None;
        let mut total_tokens: u64 = 0;
        // Per-dimension accumulation across the agentic loop so the final
        // CompletionMeta reflects the entire turn, not just the last
        // sub-request.
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cache_read: u64 = 0;
        let mut total_cache_creation: u64 = 0;

        for turn in 0..max_turns {
            let body = AnthropicRequest {
                model: model.to_string(),
                max_tokens: self.max_tokens,
                system: system_blocks.to_vec(),
                messages: messages.clone(),
                tools: tools.clone(),
                thinking: thinking_config(extended_thinking),
            };

            debug!("anthropic: POST {ANTHROPIC_API_URL} model={model} turn={turn}");

            let body_json = serde_json::to_vec(&body)
                .map_err(|e| ProviderError::Logic(format!("anthropic: serialize failed: {e}")))?;

            let resp = {
                let client = &self.client;
                let api_key = &self.api_key;
                // No anthropic-beta header: adaptive thinking rides in the
                // request body, prompt caching is GA (cache_control needs no
                // header), and the token-efficient-tools beta was removed earlier
                // (native on 4.x; the unverified string risked 400-ing tool
                // requests).
                send_with_retry("anthropic", || {
                    let req = client
                        .post(ANTHROPIC_API_URL)
                        .header("x-api-key", api_key.expose_secret().as_str())
                        .header("anthropic-version", ANTHROPIC_VERSION)
                        .header("content-type", "application/json")
                        .body(body_json.clone());
                    async move { req.send().await }
                })
                .await?
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = read_truncated_error_body(resp).await;
                return Err(
                    ProviderError::Logic(format!("anthropic returned {status}: {text}")).into(),
                );
            }

            let parsed: AnthropicResponse = resp.json().await.map_err(|e| {
                ProviderError::Logic(format!("anthropic: failed to parse response: {e}"))
            })?;

            if let Some(ref m) = parsed.model {
                last_model = Some(m.clone());
            }
            if let Some(ref u) = parsed.usage {
                let cache_read = u.cache_read_input_tokens.unwrap_or(0);
                let cache_creation = u.cache_creation_input_tokens.unwrap_or(0);
                total_tokens += u.input_tokens + u.output_tokens + cache_read + cache_creation;
                total_input_tokens += u.input_tokens;
                total_output_tokens += u.output_tokens;
                total_cache_read += cache_read;
                total_cache_creation += cache_creation;
            }

            // Check for tool_use in response.
            let has_tool_use = parsed.stop_reason.as_deref() == Some("tool_use");
            let blocks = parsed.content.unwrap_or_default();

            if has_tool_use {
                // Replay the full assistant content verbatim (thinking, text, and
                // tool_use blocks, in order). Thinking blocks that precede a
                // tool_use must be preserved or the follow-up request is rejected.
                let assistant_blocks: Vec<AnthropicContentBlock> =
                    blocks.iter().map(assistant_block_from_response).collect();
                let mut tool_result_blocks: Vec<AnthropicContentBlock> = Vec::new();

                for block in &blocks {
                    if let AnthropicResponseBlock::ToolUse { id, name, input } = block {
                        info!("anthropic: tool call [{turn}] {name} ({id})");

                        let result = executor.execute(name, input).await;

                        tool_result_blocks.push(AnthropicContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: result.content,
                            is_error: if result.is_error { Some(true) } else { None },
                        });
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

                // Stop before starting another turn once the billed spend
                // (input + output + cache writes; cache reads excluded) has
                // reached the caller's budget. A final text answer above
                // would already have returned, so nothing completed is ever
                // discarded here.
                let billed = total_input_tokens + total_output_tokens + total_cache_creation;
                if kernex_core::run::budget_exhausted(billed, token_budget) {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let usage = UsageBreakdown {
                        input_tokens: Some(total_input_tokens),
                        output_tokens: Some(total_output_tokens),
                        cache_read_tokens: if total_cache_read > 0 {
                            Some(total_cache_read)
                        } else {
                            None
                        },
                        cache_creation_tokens: if total_cache_creation > 0 {
                            Some(total_cache_creation)
                        } else {
                            None
                        },
                    };
                    let mut resp = build_response_with_usage(
                        String::new(),
                        "anthropic",
                        total_tokens,
                        elapsed_ms,
                        last_model,
                        usage,
                    );
                    resp.metadata.stop_reason = Some("budget_exhausted".to_string());
                    return Ok(resp);
                }

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
            let usage = UsageBreakdown {
                input_tokens: Some(total_input_tokens),
                output_tokens: Some(total_output_tokens),
                cache_read_tokens: if total_cache_read > 0 {
                    Some(total_cache_read)
                } else {
                    None
                },
                cache_creation_tokens: if total_cache_creation > 0 {
                    Some(total_cache_creation)
                } else {
                    None
                },
            };
            // Surface the provider stop reason (end_turn / max_tokens / ...) so
            // callers can detect a truncated reply rather than treating it as a
            // complete answer.
            let mut resp = build_response_with_usage(
                text,
                "anthropic",
                total_tokens,
                elapsed_ms,
                last_model,
                usage,
            );
            resp.metadata.stop_reason = parsed.stop_reason.clone();
            return Ok(resp);
        }

        // Max turns exhausted.
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let usage = UsageBreakdown {
            input_tokens: Some(total_input_tokens),
            output_tokens: Some(total_output_tokens),
            cache_read_tokens: if total_cache_read > 0 {
                Some(total_cache_read)
            } else {
                None
            },
            cache_creation_tokens: if total_cache_creation > 0 {
                Some(total_cache_creation)
            } else {
                None
            },
        };
        // Turn budget exhausted without a final answer. Return empty text plus
        // stop_reason="max_turns"; `Runtime::run` maps this to
        // `RunOutcome::MaxTurns` instead of persisting a synthetic answer, and
        // single-shot callers can detect it via `metadata.stop_reason`.
        let mut resp = build_response_with_usage(
            String::new(),
            "anthropic",
            total_tokens,
            elapsed_ms,
            last_model,
            usage,
        );
        resp.metadata.stop_reason = Some("max_turns".to_string());
        Ok(resp)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

/// One SSE `data:` envelope from the Anthropic Messages stream. Only the fields
/// kernex acts on are modeled; the rest are ignored.
#[derive(Deserialize)]
struct SseData {
    #[serde(rename = "type")]
    event_type: String,
    /// Present on `content_block_delta` and (for `stop_reason`) `message_delta`.
    delta: Option<SseDelta>,
    /// Present on `message_start` (carries the initial input/cache usage).
    message: Option<SseMessage>,
    /// Present on `message_delta` (carries the final output usage).
    usage: Option<SseUsage>,
    /// Present on `error` events (e.g. `overloaded_error`).
    error: Option<SseError>,
}

/// Delta payload inside `content_block_delta` (and `stop_reason` on `message_delta`).
#[derive(Deserialize)]
struct SseDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

/// The `message` object on a `message_start` event.
#[derive(Deserialize)]
struct SseMessage {
    usage: Option<SseUsage>,
}

/// Usage block as it appears in the stream.
#[derive(Deserialize)]
struct SseUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

/// The `error` object on an `error` event.
#[derive(Deserialize)]
struct SseError {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}

/// Convert one parsed SSE envelope into a [`StreamEvent`], threading running
/// `usage` state across events (the wire reports input/cache usage on
/// `message_start` and output usage + stop_reason on `message_delta`).
///
/// Returns `None` for events kernex does not surface (`ping`,
/// `content_block_start`, `content_block_stop`, and `message_start`, which only
/// updates state). `content_block_delta` text/thinking/tool-input deltas,
/// `message_delta` (-> `Usage`), `error` (-> `Error`), and `message_stop`
/// (-> `Done`) all produce events.
fn sse_data_to_event(data: SseData, usage: &mut StreamUsage) -> Option<StreamEvent> {
    match data.event_type.as_str() {
        "message_start" => {
            if let Some(u) = data.message.and_then(|m| m.usage) {
                usage.input_tokens = u.input_tokens;
                usage.cache_read_tokens = u.cache_read_input_tokens;
                usage.cache_creation_tokens = u.cache_creation_input_tokens;
            }
            None
        }
        "content_block_delta" => {
            let delta = data.delta?;
            match delta.delta_type.as_deref() {
                Some("text_delta") => delta.text.map(StreamEvent::TextDelta),
                Some("thinking_delta") => delta.thinking.map(StreamEvent::ThinkingDelta),
                Some("input_json_delta") => delta.partial_json.map(StreamEvent::InputJsonDelta),
                _ => None,
            }
        }
        "message_delta" => {
            if let Some(u) = data.usage {
                // message_delta echoes input/cache and carries the final output
                // count; prefer its non-absent values over the message_start
                // snapshot.
                if u.input_tokens > 0 {
                    usage.input_tokens = u.input_tokens;
                }
                usage.output_tokens = u.output_tokens;
                if u.cache_read_input_tokens.is_some() {
                    usage.cache_read_tokens = u.cache_read_input_tokens;
                }
                if u.cache_creation_input_tokens.is_some() {
                    usage.cache_creation_tokens = u.cache_creation_input_tokens;
                }
            }
            if let Some(sr) = data.delta.and_then(|d| d.stop_reason) {
                usage.stop_reason = Some(sr);
            }
            Some(StreamEvent::Usage(usage.clone()))
        }
        "error" => {
            let msg = data
                .error
                .map(|e| match (e.error_type, e.message) {
                    (Some(t), Some(m)) => format!("{t}: {m}"),
                    (None, Some(m)) => m,
                    (Some(t), None) => t,
                    (None, None) => "unknown error".to_string(),
                })
                .unwrap_or_else(|| "unknown error".to_string());
            Some(StreamEvent::Error(format!(
                "anthropic: stream error: {msg}"
            )))
        }
        "message_stop" => Some(StreamEvent::Done),
        // ping, content_block_start, content_block_stop: nothing to surface.
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
        let extended_thinking = context.extended_thinking;

        let messages: Vec<AnthropicMessage> = api_messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: AnthropicContent::Text(m.content.clone()),
            })
            .collect();
        ensure_no_assistant_prefill(&messages)?;

        let body = AnthropicStreamRequest {
            model: effective_model,
            max_tokens: self.max_tokens,
            stream: true,
            system: system_blocks,
            messages,
            tools: None,
            thinking: thinking_config(extended_thinking),
        };

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Logic(format!("anthropic: serialize failed: {e}")))?;

        // Build request. We send directly — send_with_retry reads the body on
        // error which would consume the stream before we can iterate it.
        // Pass the SecretString slice directly into the header builder so the
        // key never lives as an unzeroized owned String on the heap.
        // No anthropic-beta header: adaptive thinking is a body param and
        // prompt caching is GA (cache_control needs no header).
        let req = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        debug!("anthropic: POST {ANTHROPIC_API_URL} (stream=true)");

        let resp =
            req.body(body_json).send().await.map_err(|e| {
                ProviderError::Logic(format!("anthropic: stream request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = read_truncated_error_body(resp).await;
            return Err(
                ProviderError::Logic(format!("anthropic returned {status}: {text}")).into(),
            );
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        // Cap any single SSE line at 1 MiB. A misbehaving or hostile endpoint
        // that withholds newlines (or a chunked-encoding stall) would
        // otherwise grow `buffer` without bound and OOM the worker. The
        // post-drain check below bounds the RESIDUAL buffer at MAX_SSE_LINE;
        // the pre-append check bounds the transient peak at 2x even for a
        // single oversized chunk, making the cap hard rather than
        // cap-plus-one-chunk.
        const MAX_SSE_LINE: usize = 1024 * 1024;

        tokio::spawn(async move {
            use futures_util::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut buffer = String::new();
            // Running usage, assembled across `message_start` (input/cache) and
            // `message_delta` (output + stop_reason) and emitted once as a
            // `Usage` event before `Done`.
            let mut usage = StreamUsage::default();

            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        if buffer.len() + bytes.len() > MAX_SSE_LINE * 2 {
                            let _ = tx
                                .send(StreamEvent::Error(format!(
                                    "anthropic: SSE buffer would exceed {} bytes",
                                    MAX_SSE_LINE * 2
                                )))
                                .await;
                            return;
                        }
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        // Drain complete lines from the front of the buffer.
                        // `buffer.drain(..=pos)` does an in-place shift; the
                        // previous `buffer = buffer[pos+1..].to_string()`
                        // reallocated on every newline, giving O(n^2) total
                        // work over a long stream.
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].trim_end_matches('\r').to_string();
                            buffer.drain(..=pos);

                            // Anthropic does not send a `data: [DONE]` sentinel
                            // (that is an OpenAI idiom); completion is the
                            // `message_stop` event, mapped to `Done` below.
                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(sse) = serde_json::from_str::<SseData>(data) {
                                    if let Some(evt) = sse_data_to_event(sse, &mut usage) {
                                        // `Error` and `Done` are terminal: forward
                                        // then stop reading the stream.
                                        let terminal = matches!(
                                            evt,
                                            StreamEvent::Done | StreamEvent::Error(_)
                                        );
                                        if tx.send(evt).await.is_err() || terminal {
                                            return;
                                        }
                                    }
                                }
                            }
                        }

                        if buffer.len() > MAX_SSE_LINE {
                            let _ = tx
                                .send(StreamEvent::Error(format!(
                                    "anthropic: SSE line exceeded {MAX_SSE_LINE} bytes without newline"
                                )))
                                .await;
                            return;
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
            thinking: None,
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
            thinking: None,
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
    fn test_anthropic_response_parses_cache_fields() {
        let json = r#"{
            "content":[{"type":"text","text":"hi"}],
            "model":"claude-sonnet-4-20250514",
            "usage":{
                "input_tokens":12,
                "output_tokens":3,
                "cache_read_input_tokens":1024,
                "cache_creation_input_tokens":512
            },
            "stop_reason":"end_turn"
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 3);
        assert_eq!(usage.cache_read_input_tokens, Some(1024));
        assert_eq!(usage.cache_creation_input_tokens, Some(512));
    }

    #[test]
    fn test_anthropic_response_omits_cache_fields_when_absent() {
        // Responses without prompt-cache hits omit the fields entirely;
        // we must default them to None rather than 0 so downstream code can
        // distinguish "no cache info" from "cache info but zero".
        let json = r#"{"content":[{"type":"text","text":"x"}],"model":"claude-sonnet-4-20250514","usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"end_turn"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
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
            thinking: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tools"].as_array().unwrap().len(), 7);
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

    #[test]
    fn test_prefill_guard_rejects_trailing_assistant() {
        let messages = vec![
            AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("hi".into()),
            },
            AnthropicMessage {
                role: "assistant".into(),
                content: AnthropicContent::Text("partial".into()),
            },
        ];
        assert!(ensure_no_assistant_prefill(&messages).is_err());
    }

    #[test]
    fn test_prefill_guard_allows_trailing_user() {
        let messages = vec![AnthropicMessage {
            role: "user".into(),
            content: AnthropicContent::Text("hi".into()),
        }];
        assert!(ensure_no_assistant_prefill(&messages).is_ok());
    }

    #[test]
    fn test_prefill_guard_allows_empty() {
        assert!(ensure_no_assistant_prefill(&[]).is_ok());
    }

    #[test]
    fn test_adaptive_thinking_sent_when_enabled() {
        let body = AnthropicRequest {
            model: "claude-opus-4-8".into(),
            max_tokens: 8192,
            system: Vec::new(),
            messages: Vec::new(),
            tools: None,
            thinking: thinking_config(true),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
    }

    #[test]
    fn test_thinking_omitted_when_disabled() {
        let body = AnthropicRequest {
            model: "claude-opus-4-8".into(),
            max_tokens: 8192,
            system: Vec::new(),
            messages: Vec::new(),
            tools: None,
            thinking: thinking_config(false),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("thinking").is_none());
    }

    #[test]
    fn test_response_parses_thinking_block_and_extracts_only_text() {
        // A real thinking block must not break the whole parse, and its content
        // must not leak into the user-visible answer.
        let json = r#"{"content":[{"type":"thinking","thinking":"step one","signature":"sig-abc"},{"type":"text","text":"The answer."}],"model":"claude-opus-4-8","usage":{"input_tokens":3,"output_tokens":4},"stop_reason":"end_turn"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(extract_text_from_response(&resp), "The answer.");
    }

    #[test]
    fn test_response_parses_thinking_block_with_empty_field() {
        // display:"omitted" (the default) yields an empty thinking field, but the
        // block and its signature are still present and must parse.
        let json = r#"{"content":[{"type":"thinking","thinking":"","signature":"sig"},{"type":"text","text":"Hi"}],"model":"m","usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"end_turn"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(extract_text_from_response(&resp), "Hi");
    }

    #[test]
    fn test_response_parses_redacted_thinking_block() {
        let json = r#"{"content":[{"type":"redacted_thinking","data":"ENCRYPTED"},{"type":"text","text":"Done"}],"model":"m","usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"end_turn"}"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(extract_text_from_response(&resp), "Done");
    }

    #[test]
    fn test_assistant_replay_preserves_thinking_then_tooluse_in_order() {
        // After a tool_use turn the full content (thinking + text + tool_use) is
        // replayed verbatim; the thinking block keeps its signature and stays
        // first, or the API rejects the follow-up request.
        let blocks = [
            AnthropicResponseBlock::Thinking {
                thinking: "reasoning".into(),
                signature: Some("sig-1".into()),
            },
            AnthropicResponseBlock::Text {
                text: "Let me check.".into(),
            },
            AnthropicResponseBlock::ToolUse {
                id: "toolu_1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
        ];
        let replay: Vec<AnthropicContentBlock> =
            blocks.iter().map(assistant_block_from_response).collect();
        let json = serde_json::to_value(&replay).unwrap();
        assert_eq!(json[0]["type"], "thinking");
        assert_eq!(json[0]["thinking"], "reasoning");
        assert_eq!(json[0]["signature"], "sig-1");
        assert_eq!(json[1]["type"], "text");
        assert_eq!(json[2]["type"], "tool_use");
        assert_eq!(json[2]["id"], "toolu_1");
    }

    #[test]
    fn test_redacted_thinking_replays_with_data() {
        let blocks = [AnthropicResponseBlock::RedactedThinking { data: "ENC".into() }];
        let replay: Vec<AnthropicContentBlock> =
            blocks.iter().map(assistant_block_from_response).collect();
        let json = serde_json::to_value(&replay).unwrap();
        assert_eq!(json[0]["type"], "redacted_thinking");
        assert_eq!(json[0]["data"], "ENC");
    }

    // --- SSE streaming tests ---

    #[test]
    fn test_sse_stream_request_has_stream_true() {
        let body = AnthropicStreamRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 8192,
            stream: true,
            system: build_system_blocks("Be helpful."),
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: AnthropicContent::Text("Hello".into()),
            }],
            tools: None,
            thinking: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["stream"], true);
        assert!(json.get("tools").is_none());
    }

    /// Parse one SSE `data:` line through the real parser with throwaway usage state.
    fn parse_sse(raw: &str) -> Option<StreamEvent> {
        let mut usage = StreamUsage::default();
        let sse: SseData = serde_json::from_str(raw).unwrap();
        sse_data_to_event(sse, &mut usage)
    }

    #[test]
    fn test_sse_text_delta_parsed() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        match parse_sse(raw).unwrap() {
            StreamEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_sse_thinking_delta_parsed() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me reason"}}"#;
        match parse_sse(raw).unwrap() {
            StreamEvent::ThinkingDelta(t) => assert_eq!(t, "let me reason"),
            _ => panic!("expected ThinkingDelta"),
        }
    }

    #[test]
    fn test_sse_input_json_delta_parsed() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#;
        match parse_sse(raw).unwrap() {
            StreamEvent::InputJsonDelta(j) => assert_eq!(j, "{\"cmd\":"),
            _ => panic!("expected InputJsonDelta"),
        }
    }

    #[test]
    fn test_sse_message_stop_emits_done() {
        let raw = r#"{"type":"message_stop"}"#;
        assert!(matches!(parse_sse(raw).unwrap(), StreamEvent::Done));
    }

    #[test]
    fn test_sse_ping_ignored() {
        assert!(parse_sse(r#"{"type":"ping"}"#).is_none());
    }

    #[test]
    fn test_sse_content_block_boundaries_ignored() {
        assert!(parse_sse(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#
        )
        .is_none());
        assert!(parse_sse(r#"{"type":"content_block_stop","index":0}"#).is_none());
    }

    #[test]
    fn test_sse_message_delta_emits_usage_with_stop_reason() {
        // Real message_delta shape: stop_reason in `delta`, output usage top-level.
        let raw = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":15,"cache_read_input_tokens":0,"output_tokens":14}}"#;
        match parse_sse(raw).unwrap() {
            StreamEvent::Usage(u) => {
                assert_eq!(u.output_tokens, 14);
                assert_eq!(u.input_tokens, 15);
                assert_eq!(u.stop_reason.as_deref(), Some("end_turn"));
            }
            _ => panic!("expected Usage"),
        }
    }

    #[test]
    fn test_sse_message_start_then_delta_combine_usage() {
        // message_start carries input/cache; message_delta carries output +
        // stop_reason. Threaded usage must combine both.
        let mut usage = StreamUsage::default();
        let start: SseData = serde_json::from_str(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":559,"cache_read_input_tokens":40,"cache_creation_input_tokens":0,"output_tokens":2}}}"#,
        )
        .unwrap();
        assert!(sse_data_to_event(start, &mut usage).is_none());
        assert_eq!(usage.input_tokens, 559);
        assert_eq!(usage.cache_read_tokens, Some(40));

        let delta: SseData = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":83}}"#,
        )
        .unwrap();
        match sse_data_to_event(delta, &mut usage).unwrap() {
            StreamEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 559); // carried from message_start
                assert_eq!(u.output_tokens, 83); // from message_delta
                assert_eq!(u.cache_read_tokens, Some(40)); // carried
                assert_eq!(u.stop_reason.as_deref(), Some("tool_use"));
                assert_eq!(u.total_tokens(), 559 + 83 + 40);
            }
            _ => panic!("expected Usage"),
        }
    }

    #[test]
    fn test_sse_error_event_surfaced_not_swallowed() {
        // An overloaded_error must surface as Error, not look like a clean finish.
        let raw = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        match parse_sse(raw).unwrap() {
            StreamEvent::Error(e) => {
                assert!(e.contains("overloaded_error"));
                assert!(e.contains("Overloaded"));
            }
            _ => panic!("expected Error"),
        }
    }

    /// Live end-to-end streaming check against the real Messages API. Ignored by
    /// default: it makes a real, billable call and needs `ANTHROPIC_API_KEY` in
    /// the environment. Run with:
    ///   cargo test -p kernex-providers -- --ignored streaming_live
    #[tokio::test]
    #[ignore = "makes a real billable Anthropic API call; needs ANTHROPIC_API_KEY"]
    async fn streaming_live_emits_text_usage_and_done() {
        let key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.is_empty() => k,
            _ => {
                eprintln!("skipping streaming_live: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        let provider =
            AnthropicProvider::from_config(key, "claude-sonnet-4-6".into(), 64, None).unwrap();
        let ctx = Context::new("Say hello in exactly one short sentence.");
        let mut rx = provider.complete_stream(&ctx).await.expect("stream opens");

        let mut text = String::new();
        let mut usage: Option<StreamUsage> = None;
        let mut saw_done = false;
        let mut saw_error: Option<String> = None;
        while let Some(evt) = rx.recv().await {
            match evt {
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Usage(u) => usage = Some(u),
                StreamEvent::Done => {
                    saw_done = true;
                    break;
                }
                StreamEvent::Error(e) => {
                    saw_error = Some(e);
                    break;
                }
                StreamEvent::ThinkingDelta(_) | StreamEvent::InputJsonDelta(_) => {}
            }
        }

        assert!(saw_error.is_none(), "stream error: {saw_error:?}");
        assert!(saw_done, "stream never signaled Done");
        assert!(!text.trim().is_empty(), "no text streamed");
        let u = usage.expect("a Usage event must be emitted");
        assert!(u.output_tokens > 0, "output_tokens should be > 0");
        assert_eq!(u.stop_reason.as_deref(), Some("end_turn"));
        eprintln!("LIVE OK: text={text:?} usage={u:?}");
    }

    /// Live mid-loop token-budget check against the real Messages API. Ignored
    /// by default: it makes a real, billable call and needs
    /// `ANTHROPIC_API_KEY` in the environment. Run with:
    ///   cargo test -p kernex-providers -- --ignored budget_live
    #[tokio::test]
    #[ignore = "makes a real billable Anthropic API call; needs ANTHROPIC_API_KEY"]
    async fn budget_live_stops_agentic_loop_after_first_tool_turn() {
        let key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.is_empty() => k,
            _ => {
                eprintln!("skipping budget_live: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        let ws = tempfile::tempdir().unwrap();
        let provider = AnthropicProvider::from_config(
            key,
            "claude-haiku-4-5".into(),
            1024,
            Some(ws.path().to_path_buf()),
        )
        .unwrap();
        let mut ctx = Context::new(
            "Use the bash tool to run `echo budget-probe` and then summarize the output.",
        );
        ctx.max_turns = Some(8);
        // Any billed spend exhausts a budget of 1, so the loop must stop right
        // after the first tool turn instead of finishing the task.
        ctx.token_budget = Some(1);

        let resp = provider.complete(&ctx).await.expect("complete");

        assert_eq!(
            resp.metadata.stop_reason.as_deref(),
            Some("budget_exhausted"),
            "expected the loop to stop on budget after the first tool turn \
             (if the model answered without calling a tool, re-run)"
        );
        assert!(resp.metadata.tokens_used.unwrap_or(0) >= 1);
        eprintln!("LIVE OK: tokens={:?}", resp.metadata.tokens_used);
    }

    #[test]
    fn test_cache_control_present_without_beta_header() {
        // Prompt caching is driven by cache_control on the system blocks, which
        // is GA and needs no anthropic-beta header. Verify the cache_control
        // boundary still serializes (the header that used to accompany it was
        // removed; a live smoke test confirmed caching still works without it).
        let blocks = build_system_blocks("Stable.\nKERNEX_CACHE_BOUNDARY\nDynamic.");
        let json = serde_json::to_value(&blocks).unwrap();
        assert_eq!(json[0]["cache_control"]["type"], "ephemeral");
    }
}
