//! OpenCode CLI provider.
//!
//! Runs the locally installed `opencode` CLI (`opencode run --format json`)
//! as a subprocess inside the kernex sandbox. OpenCode talks to whatever
//! model provider the user configured (Anthropic, OpenAI, Ollama, OpenRouter,
//! and others), so this gives the same OS-level agent isolation as the
//! `claude-code` provider without tying it to one model vendor.
//!
//! kernex never edits the user's OpenCode config files. Per-call settings
//! (tool permissions, MCP servers) are injected through the
//! `OPENCODE_CONFIG_CONTENT` environment variable, which OpenCode merges on
//! top of its regular config.

mod command;
mod config;
mod events;
mod provider;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Default timeout for the OpenCode CLI subprocess (60 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Environment variables passed through to the OpenCode subprocess.
///
/// The sandbox clears the environment, but OpenCode is the provider itself
/// and needs the credentials of whichever model provider it is configured
/// for. Keys stored with `opencode auth login` live in OpenCode's data dir
/// and need no passthrough. The XDG variables keep OpenCode pointed at the
/// same config and data dirs the user runs it with.
const ENV_PASSTHROUGH: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "GEMINI_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
    "XAI_API_KEY",
    "FIREWORKS_API_KEY",
    "OPENCODE_API_KEY",
    "OPENCODE_CONFIG",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// OpenCode CLI provider configuration.
pub struct OpenCodeProvider {
    /// Subprocess timeout.
    timeout: Duration,
    /// Working directory for the CLI subprocess.
    working_dir: Option<PathBuf>,
    /// Default model as `provider/model` (empty = let OpenCode decide).
    model: String,
    /// Extra environment variable names to pass through to the subprocess.
    extra_env: Vec<String>,
    /// System sandbox restrictions.
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl OpenCodeProvider {
    /// Create a new OpenCode provider with default settings.
    pub fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            working_dir: None,
            model: String::new(),
            extra_env: vec![],
            sandbox_profile: Default::default(),
        }
    }

    /// Create a provider from config values.
    ///
    /// `model` uses OpenCode's `provider/model` form, for example
    /// `anthropic/claude-sonnet-4-6` or `ollama/qwen3-coder:30b`.
    pub fn from_config(timeout_secs: u64, working_dir: Option<PathBuf>, model: String) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs),
            working_dir,
            model,
            extra_env: vec![],
            sandbox_profile: Default::default(),
        }
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }

    /// Pass additional environment variables (by name) through to OpenCode,
    /// for model providers not covered by the built-in list.
    pub fn with_env_passthrough(mut self, names: Vec<String>) -> Self {
        self.extra_env = names;
        self
    }

    /// Check if the `opencode` CLI is installed and accessible.
    pub async fn check_cli() -> bool {
        Command::new("opencode")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}
