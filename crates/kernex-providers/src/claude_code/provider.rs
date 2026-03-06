//! Provider trait implementation with auto-resume logic.

use super::{mcp, ClaudeCliResponse, ClaudeCodeProvider};
use async_trait::async_trait;
use kernex_core::{
    error::KernexError,
    message::{CompletionMeta, Response},
    traits::Provider,
};
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn complete(
        &self,
        context: &kernex_core::context::Context,
    ) -> Result<Response, KernexError> {
        let prompt = context.to_prompt_string();
        let start = Instant::now();

        // Write MCP settings if any servers are declared.
        let mcp_settings_path = if !context.mcp_servers.is_empty() {
            if let Some(ref dir) = self.working_dir {
                match mcp::write_mcp_settings(dir, &context.mcp_servers).await {
                    Ok(path) => Some(path),
                    Err(e) => {
                        warn!("failed to write MCP settings: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let extra_tools = mcp::mcp_tool_patterns(&context.mcp_servers);

        // Resolve effective max_turns, allowed_tools, and model from context overrides.
        let effective_max_turns = context.max_turns.unwrap_or(self.max_turns);
        let tools_disabled = matches!(&context.allowed_tools, Some(t) if t.is_empty());
        let effective_tools: Vec<String> = context
            .allowed_tools
            .clone()
            .unwrap_or_else(|| self.allowed_tools.clone());
        let effective_model = context.model.as_deref().unwrap_or(&self.model);

        // First call with original prompt.
        let result = self
            .run_cli(
                &prompt,
                &extra_tools,
                effective_max_turns,
                &effective_tools,
                effective_model,
                tools_disabled,
                context.session_id.as_deref(),
                context.agent_name.as_deref(),
            )
            .await;

        // Always cleanup MCP settings, regardless of success or failure.
        if let Some(ref path) = mcp_settings_path {
            mcp::cleanup_mcp_settings(path).await;
        }

        let output = result?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let (mut text, mut model) = self.parse_response(&stdout, effective_max_turns);
        // CLI doesn't always echo the model back — fall back to what we requested.
        if model.is_none() && !effective_model.is_empty() {
            model = Some(effective_model.to_string());
        }

        // Auto-resume: if Claude hit max_turns and returned a session_id, retry.
        // Skip auto-resume when max_turns was explicitly set by the caller (e.g., planning calls).
        let parsed: Option<ClaudeCliResponse> = serde_json::from_str(&stdout).ok();
        if context.max_turns.is_some() {
            // Explicit max_turns override — caller controls the limit, no auto-resume.
        } else if let Some(ref resp) = parsed {
            if resp.subtype.as_deref() == Some("error_max_turns") {
                if let Some(ref session_id) = resp.session_id {
                    text = self
                        .auto_resume(
                            text,
                            session_id,
                            &extra_tools,
                            effective_max_turns,
                            &effective_tools,
                            effective_model,
                            &mut model,
                        )
                        .await;
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Capture session_id from the provider response for conversation continuity.
        let returned_session_id = parsed.as_ref().and_then(|r| r.session_id.clone());

        Ok(Response {
            text,
            metadata: CompletionMeta {
                provider_used: "claude-code".to_string(),
                tokens_used: None,
                processing_time_ms: elapsed_ms,
                model,
                session_id: returned_session_id,
            },
        })
    }

    async fn is_available(&self) -> bool {
        Self::check_cli().await
    }
}

impl ClaudeCodeProvider {
    /// Auto-resume when Claude hit max_turns and returned a session_id.
    ///
    /// Retries up to `max_resume_attempts` times, accumulating results.
    #[allow(clippy::too_many_arguments)]
    async fn auto_resume(
        &self,
        initial_text: String,
        session_id: &str,
        extra_tools: &[String],
        effective_max_turns: u32,
        effective_tools: &[String],
        effective_model: &str,
        model: &mut Option<String>,
    ) -> String {
        let mut accumulated = initial_text;
        let mut resume_session = session_id.to_string();

        for attempt in 1..=self.max_resume_attempts {
            // Exponential backoff: 2s, 4s, 8s, ... — gives the CLI session time to release.
            let delay = Duration::from_secs(2u64.pow(attempt));
            info!(
                "auto-resume: attempt {}/{} with session {} (delay {}s)",
                attempt,
                self.max_resume_attempts,
                resume_session,
                delay.as_secs()
            );
            tokio::time::sleep(delay).await;

            let resume_result = self
                .run_cli_with_session(
                    "Continue where you left off. Complete the remaining work.",
                    extra_tools,
                    &resume_session,
                    effective_max_turns,
                    effective_tools,
                    effective_model,
                )
                .await;

            match resume_result {
                Ok(resume_output) => {
                    let resume_stdout = String::from_utf8_lossy(&resume_output.stdout);
                    let (resume_text, resume_model) =
                        self.parse_response(&resume_stdout, effective_max_turns);
                    accumulated = format!("{accumulated}\n\n---\n\n{resume_text}");

                    if resume_model.is_some() {
                        *model = resume_model;
                    }

                    // Check if this resume also hit max_turns.
                    let resume_parsed: Option<ClaudeCliResponse> =
                        serde_json::from_str(&resume_stdout).ok();
                    match resume_parsed {
                        Some(ref rr) if rr.subtype.as_deref() == Some("error_max_turns") => {
                            if let Some(ref new_sid) = rr.session_id {
                                resume_session = new_sid.clone();
                                continue;
                            }
                            break;
                        }
                        _ => break,
                    }
                }
                Err(e) => {
                    warn!("auto-resume attempt {} failed: {e}", attempt);
                    continue;
                }
            }
        }

        accumulated
    }
}
