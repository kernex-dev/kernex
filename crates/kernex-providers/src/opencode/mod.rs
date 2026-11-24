//! OpenCode CLI provider.
//!
//! Runs the locally installed `opencode` CLI (`opencode run --format json`)
//! as a subprocess inside the kernex sandbox. OpenCode talks to whatever
//! model provider the user configured (Anthropic, OpenAI, Ollama, OpenRouter,
//! and others), so this gives the same OS-level agent isolation as the
//! `claude-code` provider without tying it to one model vendor.
//!
//! OpenCode runs with its own config, data, state and cache directories
//! inside the runtime data directory ([`OpenCodeProvider::cli_state_dir`]),
//! never the user's, and reads no project config. Its tools run in the same
//! sandbox as OpenCode itself, so nothing they write can add a plugin, an
//! agent or a remote config that the user's own OpenCode would later run
//! outside the sandbox. Per-call settings (tool permissions, MCP servers)
//! are injected through the `OPENCODE_CONFIG_CONTENT` environment variable.
//! A model provider's key reaches OpenCode only through its environment
//! variable (see [`OpenCodeProvider::with_env_passthrough`] for providers
//! kernex does not know): logins made with `opencode auth login` are not
//! used.

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

/// The XDG variables OpenCode locates its files with, and the directory
/// under [`OpenCodeProvider::cli_state_dir`] each one points to.
const XDG_DIRS: &[(&str, &str)] = &[
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
];

/// The API key variables of the model provider named by the `provider/`
/// prefix of an OpenCode model.
///
/// The sandbox clears the environment, and OpenCode gets back only the key
/// of the upstream it is configured to call, so a tool call it runs cannot
/// read the others. A provider not listed here
/// gets no key; name its variable with
/// [`OpenCodeProvider::with_env_passthrough`].
fn provider_key_env(model: &str) -> &'static [&'static str] {
    let Some((provider, _)) = model.split_once('/') else {
        return &[];
    };
    match provider {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "google" => &["GOOGLE_GENERATIVE_AI_API_KEY", "GEMINI_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "fireworks-ai" | "fireworks" => &["FIREWORKS_API_KEY"],
        "opencode" => &["OPENCODE_API_KEY"],
        _ => &[],
    }
}

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
    sandbox_dirs: crate::SandboxDirs,
    /// Allow every OpenCode permission when no tool list applies.
    permission_bypass: bool,
    /// Which hosts OpenCode's `webfetch` may reach.
    web_fetch: crate::WebFetchPolicy,
    /// The program started (`opencode`, found on `PATH`).
    program: String,
    /// Reads a variable of the host environment (replaced in tests).
    env_source: fn(&str) -> Option<String>,
}

fn host_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
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
            sandbox_dirs: Default::default(),
            permission_bypass: false,
            web_fetch: Default::default(),
            program: "opencode".to_string(),
            env_source: host_env,
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
            sandbox_dirs: Default::default(),
            permission_bypass: false,
            web_fetch: Default::default(),
            program: "opencode".to_string(),
            env_source: host_env,
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

    /// Pass additional environment variables (by name) through to OpenCode,
    /// for model providers not covered by the built-in list. They are passed
    /// whatever model is configured.
    pub fn with_env_passthrough(mut self, names: Vec<String>) -> Self {
        self.extra_env = names;
        self
    }

    /// Allow every OpenCode permission for requests that name no tools.
    ///
    /// Off by default: such a request may then use only the read-only tools
    /// (`read`, `glob`, `grep`, `list`, `lsp`, `todowrite`). A request with
    /// a tool list never gets the bypass. The OS sandbox applies either way.
    pub fn with_permission_bypass(mut self, bypass: bool) -> Self {
        self.permission_bypass = bypass;
        self
    }

    /// Set the `web_fetch` policy. OpenCode has no per-host fetch rule, so
    /// its `webfetch` and `websearch` tools run only under
    /// [`WebFetchPolicy::AllowAll`](crate::WebFetchPolicy::AllowAll). The
    /// default is disabled.
    pub fn with_web_fetch_policy(mut self, policy: crate::WebFetchPolicy) -> Self {
        self.web_fetch = policy;
        self
    }

    /// Start `program` instead of `opencode` (tests use a stand-in CLI).
    #[cfg(test)]
    pub(crate) fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// Read host variables from `source` instead of the process environment.
    #[cfg(test)]
    pub(crate) fn with_env_source(mut self, source: fn(&str) -> Option<String>) -> Self {
        self.env_source = source;
        self
    }

    /// The directory OpenCode keeps its config, data, state and cache in
    /// when kernex runs it: `cli/opencode` inside the runtime data directory.
    /// The `config` directory inside it is read-only to OpenCode, and its
    /// login files are closed, so OpenCode reaches a model provider only
    /// through the provider's API key variable.
    pub fn cli_state_dir(&self) -> PathBuf {
        self.sandbox_dirs
            .resolved_data_dir()
            .join("cli")
            .join("opencode")
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
