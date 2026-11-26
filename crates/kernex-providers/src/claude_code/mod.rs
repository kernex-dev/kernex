//! Claude Code CLI provider.
//!
//! Uses the locally installed `claude` CLI as a subprocess, inside the OS
//! sandbox and with its own config directory
//! ([`ClaudeCodeProvider::cli_config_dir`]). It authenticates with an OAuth
//! token passed to [`ClaudeCodeProvider::from_config`], `ANTHROPIC_API_KEY`,
//! or a login made with that config directory.

mod command;
mod mcp;
mod permissions;
mod provider;
mod response;

#[cfg(test)]
mod tests;

use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

pub use mcp::mcp_tool_patterns;

/// Default timeout for Claude Code CLI subprocess (60 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Claude Code CLI provider configuration.
pub struct ClaudeCodeProvider {
    /// Maximum agentic turns per invocation.
    max_turns: u32,
    /// Tools the CLI is offered when the request names none. `None` offers
    /// every tool; see [`ClaudeCodeProvider::with_permission_bypass`].
    allowed_tools: Option<Vec<String>>,
    /// Subprocess timeout.
    timeout: Duration,
    /// Working directory for the CLI subprocess.
    working_dir: Option<PathBuf>,
    /// Max auto-resume attempts when Claude hits max_turns.
    max_resume_attempts: u32,
    /// Default model to pass via `--model` (empty = let CLI decide).
    model: String,
    /// OAuth token injected as `CLAUDE_CODE_OAUTH_TOKEN` env var.
    oauth_token: Option<String>,
    /// System sandbox restrictions.
    sandbox_profile: kernex_sandbox::SandboxProfile,
    sandbox_dirs: crate::SandboxDirs,
    /// Run the CLI with its permission checks off when no tool list applies.
    permission_bypass: bool,
    /// Which hosts the CLI's `WebFetch` may reach.
    web_fetch: crate::WebFetchPolicy,
    /// The program started (`claude`, found on `PATH`).
    program: String,
}

/// JSON response from `claude -p --output-format json`.
#[derive(Debug, Deserialize)]
struct ClaudeCliResponse {
    /// "result"
    #[serde(default, rename = "type")]
    response_type: Option<String>,
    /// "success", "error_max_turns", etc.
    #[serde(default)]
    subtype: Option<String>,
    /// The actual text response.
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    num_turns: Option<u32>,
}

impl ClaudeCodeProvider {
    /// Create a new Claude Code provider with default settings.
    pub fn new() -> Self {
        Self {
            max_turns: 25,
            allowed_tools: None,
            timeout: DEFAULT_TIMEOUT,
            working_dir: None,
            max_resume_attempts: 5,
            model: String::new(),
            oauth_token: None,
            sandbox_profile: Default::default(),
            sandbox_dirs: Default::default(),
            permission_bypass: false,
            web_fetch: Default::default(),
            program: "claude".to_string(),
        }
    }

    /// Create a provider from config values.
    ///
    /// `allowed_tools` is used when a request names no tools itself.
    /// `Some(list)` offers exactly the tools it names, and `Some(vec![])`
    /// offers none. `None` sets no default: such a request is offered every
    /// tool, under the CLI's permission checks unless
    /// [`with_permission_bypass`](Self::with_permission_bypass) is set.
    pub fn from_config(
        max_turns: u32,
        allowed_tools: Option<Vec<String>>,
        timeout_secs: u64,
        working_dir: Option<PathBuf>,
        max_resume_attempts: u32,
        model: String,
        oauth_token: Option<String>,
    ) -> Self {
        Self {
            max_turns,
            allowed_tools,
            timeout: Duration::from_secs(timeout_secs),
            working_dir,
            max_resume_attempts,
            model,
            oauth_token,
            sandbox_profile: Default::default(),
            sandbox_dirs: Default::default(),
            permission_bypass: false,
            web_fetch: Default::default(),
            program: "claude".to_string(),
        }
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

    /// Run the CLI with its permission checks off
    /// (`--dangerously-skip-permissions`) for requests that name no tools.
    ///
    /// Off by default: such a request then runs in the CLI's `dontAsk`
    /// mode, where only its read-only tools, the declared MCP servers' tools
    /// and the fetches the `web_fetch` policy allows run. A request with a
    /// tool list never runs with the bypass. The OS sandbox applies either
    /// way.
    pub fn with_permission_bypass(mut self, bypass: bool) -> Self {
        self.permission_bypass = bypass;
        self
    }

    /// Set the `web_fetch` policy. The CLI's `WebFetch` and `WebSearch`
    /// run only under [`WebFetchPolicy::AllowAll`](crate::WebFetchPolicy::AllowAll):
    /// the CLI approves a built-in list of documentation hosts by itself, so
    /// a host list cannot be enforced and turns them off. The default is
    /// disabled. The CLI keeps the network it needs to reach the model, so
    /// this governs the fetch tools, not what a shell command can reach.
    pub fn with_web_fetch_policy(mut self, policy: crate::WebFetchPolicy) -> Self {
        self.web_fetch = policy;
        self
    }

    /// Start `program` instead of `claude` (tests use a stand-in CLI).
    #[cfg(test)]
    pub(crate) fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// Check if the `claude` CLI is installed and accessible.
    pub async fn check_cli() -> bool {
        Command::new("claude")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Default for ClaudeCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}
