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

    /// Returns a list of all supported provider names.
    pub fn supported_providers() -> &'static [&'static str] {
        &[
            "openai",
            "anthropic",
            "gemini",
            "ollama",
            "openrouter",
            "claude-code",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_config_default() {
        let config = ProviderConfig::default();
        assert!(config.base_url.is_none());
        assert!(config.api_key.is_none());
        assert!(config.model.is_none());
        assert!(config.max_tokens.is_none());
        assert!(config.workspace_path.is_none());
    }

    #[test]
    fn provider_config_with_values() {
        let config = ProviderConfig {
            base_url: Some("https://api.example.com".to_string()),
            api_key: Some("sk-test".to_string()),
            model: Some("gpt-4".to_string()),
            max_tokens: Some(4096),
            workspace_path: Some(PathBuf::from("/tmp")),
            sandbox_profile: None,
        };
        assert_eq!(config.base_url, Some("https://api.example.com".to_string()));
        assert_eq!(config.model, Some("gpt-4".to_string()));
    }

    #[test]
    fn factory_unknown_provider_error() {
        let result = ProviderFactory::create("unknown-provider", ProviderConfig::default());
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Unknown provider type"));
        }
    }

    #[test]
    fn factory_supported_providers_list() {
        let providers = ProviderFactory::supported_providers();
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"gemini"));
        assert!(providers.contains(&"ollama"));
        assert!(providers.contains(&"openrouter"));
        assert!(providers.contains(&"claude-code"));
        assert_eq!(providers.len(), 6);
    }

    #[test]
    fn factory_case_insensitive() {
        // Unknown provider should fail regardless of case
        let result = ProviderFactory::create("UNKNOWN", ProviderConfig::default());
        assert!(result.is_err());
    }

    #[test]
    fn factory_creates_openai() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("openai", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn factory_creates_anthropic() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("anthropic", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn factory_creates_gemini() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("gemini", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "gemini");
    }

    #[test]
    fn factory_creates_ollama() {
        let config = ProviderConfig {
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("ollama", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn factory_creates_openrouter() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("openrouter", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "openrouter");
    }

    #[test]
    fn factory_creates_claude_code() {
        let config = ProviderConfig {
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("claude-code", config);
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name(), "claude-code");
    }
}
