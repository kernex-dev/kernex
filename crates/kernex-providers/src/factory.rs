use kernex_core::error::KernexError;
use kernex_core::traits::Provider;
use std::path::PathBuf;

/// Configuration for dynamically creating a provider.
#[derive(Default, Clone, Debug)]
pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub workspace_path: Option<PathBuf>,
    pub sandbox_profile: Option<kernex_sandbox::SandboxProfile>,
}

/// Factory to instantiate providers by string name.
pub struct ProviderFactory;

impl ProviderFactory {
    /// Create a provider from a string name ("openai", "anthropic", "gemini", "ollama", "openrouter", "claude-code").
    pub fn create(
        provider: &str,
        config: ProviderConfig,
    ) -> Result<Box<dyn Provider>, KernexError> {
        match provider.to_lowercase().as_str() {
            "openai" => {
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    config.model.unwrap_or_else(|| "gpt-4o".to_string()),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "anthropic" => {
                let p = crate::anthropic::AnthropicProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    config
                        .model
                        .unwrap_or_else(|| "claude-3-7-sonnet-20250219".to_string()),
                    config.max_tokens.unwrap_or(8192),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "gemini" => {
                let p = crate::gemini::GeminiProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    config
                        .model
                        .unwrap_or_else(|| "gemini-2.5-flash".to_string()),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "ollama" => {
                let p = crate::ollama::OllamaProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "http://localhost:11434".to_string()),
                    config.model.unwrap_or_else(|| "llama3.2".to_string()),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "openrouter" => {
                let p = crate::openrouter::OpenRouterProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    config
                        .model
                        .unwrap_or_else(|| "anthropic/claude-3.5-sonnet".to_string()),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "claude-code" => {
                let p = crate::claude_code::ClaudeCodeProvider::from_config(
                    25,     // default turns
                    vec![], // allowed tools
                    3600,   // timeout
                    config.workspace_path,
                    5, // max resumes
                    config.model.unwrap_or_default(),
                    None, // oauth token
                )
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            _ => Err(KernexError::Provider(format!(
                "Unknown provider type: {}",
                provider
            ))),
        }
    }
}
