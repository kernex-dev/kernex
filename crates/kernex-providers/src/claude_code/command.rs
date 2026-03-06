//! CLI command building and subprocess execution.

use super::ClaudeCodeProvider;
use kernex_core::error::KernexError;
use tokio::process::Command;
use tracing::debug;

impl ClaudeCodeProvider {
    /// Build the CLI argument list for `run_cli()`.
    ///
    /// Extracted as a pure function so argument construction is testable
    /// without subprocess execution. Returns `Vec<String>` of CLI arguments
    /// (excluding the binary name).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_run_cli_args(
        prompt: &str,
        extra_allowed_tools: &[String],
        max_turns: u32,
        allowed_tools: &[String],
        model: &str,
        context_disabled_tools: bool,
        session_id: Option<&str>,
        agent_name: Option<&str>,
    ) -> Vec<String> {
        let mut args = Vec::new();

        // Agent mode: --agent <name> before -p.
        // When agent_name is set, skip --resume (agent mode does not use sessions).
        // Reject agent names containing path separators or traversal patterns
        // to prevent path traversal attacks via the --agent flag.
        let agent = agent_name
            .filter(|n| !n.is_empty())
            .filter(|n| !n.contains('/') && !n.contains('\\') && !n.contains(".."));

        if let Some(name) = agent {
            args.push("--agent".to_string());
            args.push(name.to_string());
        }
        let use_agent = agent.is_some();

        args.push("-p".to_string());
        args.push(prompt.to_string());

        args.push("--output-format".to_string());
        args.push("json".to_string());

        args.push("--max-turns".to_string());
        args.push(max_turns.to_string());

        // Model override.
        if !model.is_empty() {
            args.push("--model".to_string());
            args.push(model.to_string());
        }

        // Session continuity: --resume resumes an existing conversation by session ID.
        // Skipped when agent_name is set (agent mode does not use sessions).
        if !use_agent {
            if let Some(sid) = session_id {
                args.push("--resume".to_string());
                args.push(sid.to_string());
            }
        }

        // Tool permissions: In `-p` (non-interactive) mode, Claude Code
        // cannot prompt for approval — tools must be pre-approved or
        // permissions bypassed entirely.
        //
        // - Agent mode -> always bypass (agent frontmatter controls tools).
        // - `context_disabled_tools` = caller wants NO tools (classification).
        // - `allowed_tools` empty = full access intended -> bypass.
        // - `allowed_tools` non-empty = explicit whitelist -> pre-approve only those.
        if use_agent {
            args.push("--dangerously-skip-permissions".to_string());
        } else if context_disabled_tools {
            args.push("--allowedTools".to_string());
            args.push(String::new());
        } else if allowed_tools.is_empty() {
            args.push("--dangerously-skip-permissions".to_string());
            // MCP tool patterns still needed so Claude knows about them.
            for tool in extra_allowed_tools {
                args.push("--allowedTools".to_string());
                args.push(tool.clone());
            }
        } else {
            for tool in allowed_tools {
                args.push("--allowedTools".to_string());
                args.push(tool.clone());
            }
            for tool in extra_allowed_tools {
                args.push("--allowedTools".to_string());
                args.push(tool.clone());
            }
        }

        args
    }

    /// Run the claude CLI subprocess with a timeout.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_cli(
        &self,
        prompt: &str,
        extra_allowed_tools: &[String],
        max_turns: u32,
        allowed_tools: &[String],
        model: &str,
        context_disabled_tools: bool,
        session_id: Option<&str>,
        agent_name: Option<&str>,
    ) -> Result<std::process::Output, KernexError> {
        let mut cmd = self.base_command();

        let args = Self::build_run_cli_args(
            prompt,
            extra_allowed_tools,
            max_turns,
            allowed_tools,
            model,
            context_disabled_tools,
            session_id,
            agent_name,
        );
        cmd.args(&args);

        debug!(
            "executing: claude {}",
            if agent_name.is_some() {
                "--agent <name> -p <prompt>"
            } else {
                "-p <prompt>"
            }
        );
        self.execute_with_timeout(cmd, "claude CLI").await
    }

    /// Run the claude CLI subprocess with a specific session ID (for auto-resume).
    pub(super) async fn run_cli_with_session(
        &self,
        prompt: &str,
        extra_allowed_tools: &[String],
        session_id: &str,
        max_turns: u32,
        allowed_tools: &[String],
        model: &str,
    ) -> Result<std::process::Output, KernexError> {
        let mut cmd = self.base_command();

        cmd.arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("json")
            .arg("--max-turns")
            .arg(max_turns.to_string())
            .arg("--resume")
            .arg(session_id);

        // Model override.
        if !model.is_empty() {
            cmd.arg("--model").arg(model);
        }

        // Same permission logic as run_cli: bypass when full access,
        // otherwise pre-approve only the listed tools.
        if allowed_tools.is_empty() {
            cmd.arg("--dangerously-skip-permissions");
            for tool in extra_allowed_tools {
                cmd.arg("--allowedTools").arg(tool);
            }
        } else {
            for tool in allowed_tools {
                cmd.arg("--allowedTools").arg(tool);
            }
            for tool in extra_allowed_tools {
                cmd.arg("--allowedTools").arg(tool);
            }
        }

        debug!("executing: claude -p <resume> --resume {session_id}");
        self.execute_with_timeout(cmd, "claude CLI resume").await
    }

    /// Build the base `Command` with working directory and system protection.
    fn base_command(&self) -> Command {
        let mut cmd = match self.working_dir {
            Some(ref dir) => {
                // Protection blocks writes to data dir (parent of workspace)
                // so memory.db is safe, but skills, projects, etc. are writable.
                let data_dir = dir.parent().unwrap_or(dir);
                let mut c = kernex_sandbox::protected_command("claude", data_dir);
                c.current_dir(dir);
                c
            }
            None => Command::new("claude"),
        };
        // Remove CLAUDECODE env var so the CLI doesn't think it's nested.
        cmd.env_remove("CLAUDECODE");
        // Inject OAuth token if configured.
        if let Some(ref token) = self.oauth_token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }
        cmd
    }

    /// Execute a command with the configured timeout and standard error handling.
    async fn execute_with_timeout(
        &self,
        mut cmd: Command,
        label: &str,
    ) -> Result<std::process::Output, KernexError> {
        let output = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| {
                KernexError::Provider(format!(
                    "{label} timed out after {}s",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|e| KernexError::Provider(format!("failed to run {label}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(KernexError::Provider(format!(
                "{label} exited with {}: {stderr}",
                output.status
            )));
        }

        Ok(output)
    }
}
