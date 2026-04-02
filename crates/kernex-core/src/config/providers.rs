use serde::{Deserialize, Serialize};

use super::defaults::*;

/// Provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(default = "default_provider")]
    pub default: String,
    #[serde(default, rename = "claude-code")]
    pub claude_code: Option<ClaudeCodeConfig>,
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAiConfig>,
    pub ollama: Option<OllamaConfig>,
    pub openrouter: Option<OpenRouterConfig>,
    pub gemini: Option<GeminiConfig>,
}

/// Claude Code CLI provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_allowed_tools")]
    pub allowed_tools: Vec<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_resume_attempts")]
    pub max_resume_attempts: u32,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_model_complex")]
    pub model_complex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_token: Option<String>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_turns: default_max_turns(),
            allowed_tools: vec![],
            timeout_secs: default_timeout_secs(),
            max_resume_attempts: default_max_resume_attempts(),
            model: default_model(),
            model_complex: default_model_complex(),
            oauth_token: None,
        }
    }
}

/// Anthropic API provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_anthropic_model")]
    pub model: String,
    #[serde(default = "default_anthropic_max_tokens")]
    pub max_tokens: u32,
    /// HTTP request timeout in seconds (default: 120).
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_anthropic_max_tokens() -> u32 {
    8192
}

/// OpenAI-compatible provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_openai_model")]
    pub model: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    /// HTTP request timeout in seconds (default: 120).
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
}

/// Ollama local provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ollama_base_url")]
    pub base_url: String,
    #[serde(default = "default_ollama_model")]
    pub model: String,
    /// HTTP request timeout in seconds (default: 120).
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
}

/// OpenRouter proxy config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// HTTP request timeout in seconds (default: 120).
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
}

/// Google Gemini API provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_gemini_model")]
    pub model: String,
    /// HTTP request timeout in seconds (default: 120).
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
}
