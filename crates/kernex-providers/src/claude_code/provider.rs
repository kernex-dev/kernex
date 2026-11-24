//! Provider trait implementation with auto-resume logic.

use super::command::CliCall;
use super::permissions::ToolAccess;
use super::{mcp, ClaudeCliResponse, ClaudeCodeProvider};
use crate::cli_support::{self, CliWorkspace};
use crate::error::ProviderError;
use async_trait::async_trait;
use kernex_core::{
    error::KernexError,
    message::{CompletionMeta, Response},
    traits::Provider,
};
use std::path::Path;
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

        // Without a configured working directory the call gets an empty
        // temporary one, so the CLI always runs inside the sandbox. It lives
        // until the last auto-resume returns.
        let workspace = CliWorkspace::for_call(self.working_dir.as_deref()).map_err(|e| {
            ProviderError::Logic(format!("cannot create a workspace for claude CLI: {e}"))
        })?;

        // The request's tool list wins; then the provider's; `None` offers
        // every tool.
        let allowed: Option<Vec<String>> = context
            .allowed_tools
            .clone()
            .or_else(|| self.allowed_tools.clone());
        let servers = cli_support::servers_for_cli(
            &context.mcp_servers,
            allowed.as_deref(),
            workspace.path(),
            "claude-code",
        );
        let server_names: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();

        // Write the servers the CLI may start to a throwaway temp file passed
        // via `--mcp-config`. Never touches the user's config.
        let mcp_config_path = if servers.is_empty() {
            None
        } else {
            match mcp::write_mcp_config_tempfile(&self.mcp_config_dir(), &servers).await {
                Ok(path) => Some(path),
                Err(e) => {
                    warn!("failed to write MCP config: {e}");
                    None
                }
            }
        };

        let effective_max_turns = context.max_turns.unwrap_or(self.max_turns);
        let effective_model = context.model.as_deref().unwrap_or(&self.model);
        let call = CliCall {
            prompt: &prompt,
            max_turns: effective_max_turns,
            model: effective_model,
            session_id: context.session_id.as_deref(),
            agent_name: context.agent_name.as_deref(),
            access: ToolAccess {
                allowed: allowed.as_deref(),
                bypass: self.permission_bypass,
                web_fetch: &self.web_fetch,
                mcp_servers: &server_names,
            },
            mcp_config_path: mcp_config_path.as_deref(),
        };

        // First call with original prompt.
        let result = self.run_cli(&call, workspace.path()).await;

        // The temp MCP config must outlive auto-resume (resumes reuse it), so
        // clean it up at the end. On an error here, clean up before returning.
        let output = match result {
            Ok(o) => o,
            Err(e) => {
                if let Some(ref path) = mcp_config_path {
                    mcp::cleanup_mcp_config(path).await;
                }
                return Err(e);
            }
        };
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
                        .auto_resume(text, session_id, &call, workspace.path(), &mut model)
                        .await;
                }
            }
        }

        // All CLI calls (first + any resumes) are done; remove the temp config.
        if let Some(ref path) = mcp_config_path {
            mcp::cleanup_mcp_config(path).await;
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
                ..Default::default()
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
    /// Each resume runs with the permissions, model and MCP config of
    /// `first`, in the same workspace.
    async fn auto_resume(
        &self,
        initial_text: String,
        session_id: &str,
        first: &CliCall<'_>,
        workspace: &Path,
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

            let call = CliCall {
                prompt: "Continue where you left off. Complete the remaining work.",
                session_id: Some(&resume_session),
                agent_name: None,
                access: first.access,
                ..*first
            };
            let resume_result = self.run_cli(&call, workspace).await;

            match resume_result {
                Ok(resume_output) => {
                    let resume_stdout = String::from_utf8_lossy(&resume_output.stdout);
                    let (resume_text, resume_model) =
                        self.parse_response(&resume_stdout, first.max_turns);
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
