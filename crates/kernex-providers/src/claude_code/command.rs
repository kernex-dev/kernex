//! CLI command building and subprocess execution.

use super::permissions::{permission_args, ToolAccess};
use super::ClaudeCodeProvider;
use crate::error::ProviderError;
use kernex_core::error::KernexError;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// True if `s` would be parsed by the `claude` CLI's argv parser as an
/// option rather than the value we intended. Used to reject context- or
/// skill-supplied strings that start with `-` so they cannot smuggle a
/// flag like `--system-prompt=evil` into the subprocess.
fn looks_like_cli_flag(s: &str) -> bool {
    s.starts_with('-')
}

/// One CLI invocation: the request half of the argument list.
#[derive(Clone, Copy)]
pub(super) struct CliCall<'a> {
    pub prompt: &'a str,
    pub max_turns: u32,
    pub model: &'a str,
    pub session_id: Option<&'a str>,
    pub agent_name: Option<&'a str>,
    pub access: ToolAccess<'a>,
    pub mcp_config_path: Option<&'a Path>,
}

impl ClaudeCodeProvider {
    /// Build the CLI argument list (excluding the binary name).
    ///
    /// Pure so argument construction is testable without a subprocess. The
    /// first call and every auto-resume go through here, so a resume runs
    /// with the same permissions as the call it continues.
    pub(super) fn build_run_cli_args(call: &CliCall<'_>) -> Vec<String> {
        let mut args = Vec::new();

        // Agent mode: --agent <name> before -p.
        // When agent_name is set, skip --resume (agent mode does not use sessions).
        // Reject agent names containing path separators, traversal patterns,
        // or a leading `-` so a poisoned skill can't smuggle a flag (e.g.
        // `agent_name = "--system-prompt=evil"`) into the CLI's argv.
        let agent = call
            .agent_name
            .filter(|n| !n.is_empty())
            .filter(|n| !n.contains('/') && !n.contains('\\') && !n.contains(".."))
            .filter(|n| !looks_like_cli_flag(n));

        if let Some(name) = agent {
            args.push("--agent".to_string());
            args.push(name.to_string());
        }

        // `-p` (print mode) takes no value; the prompt is the positional
        // argument, passed last after `--` so a prompt that starts with `-`
        // is never read as an option and a list-valued option before it
        // cannot take it in.
        args.push("-p".to_string());

        args.push("--output-format".to_string());
        args.push("json".to_string());

        args.push("--max-turns".to_string());
        args.push(call.max_turns.to_string());

        // Reject values that look like CLI flags (`--system-prompt=evil`,
        // `-h`) so context-poisoning via `context.model` cannot inject
        // arbitrary flags into the claude subprocess.
        if !call.model.is_empty() && !looks_like_cli_flag(call.model) {
            args.push("--model".to_string());
            args.push(call.model.to_string());
        }

        // Session continuity. Skipped in agent mode (no sessions there).
        if agent.is_none() {
            if let Some(sid) = call.session_id.filter(|s| !looks_like_cli_flag(s)) {
                args.push("--resume".to_string());
                args.push(sid.to_string());
            }
        }

        args.extend(permission_args(&call.access));

        // kernex's declared MCP servers come from a temp file via the CLI's
        // own `--mcp-config` mechanism, never from the user's config.
        if let Some(p) = call.mcp_config_path {
            args.push("--mcp-config".to_string());
            args.push(p.to_string_lossy().into_owned());
        }
        args.push("--".to_string());
        args.push(call.prompt.to_string());
        args
    }

    /// Run the claude CLI subprocess in `workspace` with a timeout.
    pub(super) async fn run_cli(
        &self,
        call: &CliCall<'_>,
        workspace: &Path,
    ) -> Result<std::process::Output, KernexError> {
        let mut cmd = self.base_command(workspace)?;
        cmd.args(Self::build_run_cli_args(call));
        debug!(
            "executing: claude {}",
            match (call.agent_name.is_some(), call.session_id.is_some()) {
                (true, _) => "--agent <name> -p <prompt>",
                (false, true) => "-p <prompt> --resume <session>",
                (false, false) => "-p <prompt>",
            }
        );
        self.execute_with_timeout(cmd, "claude CLI").await
    }

    /// The directory the CLI keeps its settings, login and transcripts in
    /// when kernex runs it: `cli/claude` inside the runtime data directory.
    ///
    /// The CLI's own tools run in the same sandbox as the CLI, so they can
    /// write wherever the CLI can. Keeping the CLI out of the user's
    /// `~/.claude` and `~/.claude.json` means a tool call cannot add a hook
    /// or an MCP server that the user's next interactive session would run
    /// outside the sandbox. A user who logged in to the CLI interactively is
    /// not logged in here: pass an OAuth token (`claude setup-token`) to
    /// [`ClaudeCodeProvider::from_config`], set `ANTHROPIC_API_KEY`, or log in
    /// once with `CLAUDE_CONFIG_DIR` set to this directory.
    pub fn cli_config_dir(&self) -> PathBuf {
        self.sandbox_dirs
            .resolved_data_dir()
            .join("cli")
            .join("claude")
    }

    /// Where the per-call MCP config files are written: `cli/mcp-config`
    /// inside the data directory, which no sandboxed process can write.
    /// Created owner-only.
    pub(super) fn mcp_config_dir(&self) -> PathBuf {
        let dir = self
            .sandbox_dirs
            .resolved_data_dir()
            .join("cli")
            .join("mcp-config");
        if let Err(e) = crate::cli_support::create_private_dir(&dir) {
            tracing::warn!("mcp: cannot create {}: {e}", dir.display());
        }
        dir
    }

    /// Build the base `Command`: the CLI sandboxed in `workspace`, with its
    /// own config directory, network access and its credentials.
    ///
    /// Errors when OS-level sandbox enforcement is required (profile flag or
    /// `KERNEX_REQUIRE_SANDBOX=1`) and the host cannot provide it.
    fn base_command(&self, workspace: &Path) -> Result<Command, KernexError> {
        let config_dir = self.cli_config_dir();
        crate::cli_support::create_private_dir(&config_dir).map_err(|e| {
            ProviderError::Logic(format!(
                "refusing to run claude CLI: cannot create {}: {e}",
                config_dir.display()
            ))
        })?;
        // The working directory and the CLI's config directory are the only
        // writable parts of the scope; the rest of the data dir stays out
        // of the CLI's reach.
        let scope = self.sandbox_dirs.scope(workspace);
        // The Claude CLI IS the provider: it must reach the API, so it is
        // exempt from the subprocess egress deny-by-default that governs
        // agent tool subprocesses.
        let mut profile = self.sandbox_profile.clone();
        profile.allow_network = true;
        profile.allowed_paths.push(config_dir.clone());
        // A login made with this config dir is kept in the macOS keychain.
        profile.allow_keychain = true;
        let mut cmd = kernex_sandbox::try_protected_command(&self.program, &scope, &profile)
            .map_err(|e| ProviderError::Logic(format!("refusing to run claude CLI: {e}")))?;
        cmd.current_dir(workspace);
        cmd.env("CLAUDE_CONFIG_DIR", &config_dir);
        // The Claude CLI is the provider subprocess itself: pass its auth
        // through explicitly. The hardened env cleared it along with
        // everything else; the CLI accepts either its OAuth token or an
        // Anthropic API key.
        if let Some(ref token) = self.oauth_token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                cmd.env("ANTHROPIC_API_KEY", key);
            }
        }
        Ok(cmd)
    }

    /// Execute a command with the configured timeout and standard error handling.
    async fn execute_with_timeout(
        &self,
        cmd: Command,
        label: &str,
    ) -> Result<std::process::Output, KernexError> {
        let output = crate::cli_support::run_with_timeout(cmd, self.timeout)
            .await
            .map_err(|_| {
                ProviderError::Logic(format!(
                    "{label} timed out after {}s",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|e| ProviderError::Logic(format!("failed to run {label}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ProviderError::Logic(format!(
                "{label} exited with {}: {stderr}",
                output.status
            ))
            .into());
        }

        Ok(output)
    }
}
