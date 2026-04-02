use kernex_core::error::KernexError;
use kernex_core::run::ModelTier;
use kernex_core::traits::Provider;
use std::path::PathBuf;

/// Configuration for dynamically creating a provider.
#[derive(Default, Clone, Debug)]
pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    /// Explicit model name. Takes precedence over `tier`.
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub workspace_path: Option<PathBuf>,
    pub sandbox_profile: Option<kernex_sandbox::SandboxProfile>,
    /// Performance/cost tier used to select a model when `model` is `None`.
    pub tier: Option<ModelTier>,
}

/// Resolve the model name: explicit `model` wins; else derive from `tier`; else `None`.
///
/// A `None` return means the caller's hardcoded default should apply.
fn resolve_model(provider: &str, model: Option<String>, tier: Option<ModelTier>) -> Option<String> {
    model.or_else(|| {
        tier.map(|t| model_from_tier(provider, t).to_string())
            .filter(|s| !s.is_empty())
    })
}

/// Map a provider name + tier to a concrete model identifier.
fn model_from_tier(provider: &str, tier: ModelTier) -> &'static str {
    match (provider, tier) {
        ("openai", ModelTier::Standard) => "gpt-4o-mini",
        ("openai", ModelTier::Flagship) => "gpt-4o",
        ("anthropic", ModelTier::Standard) => "claude-sonnet-4-6",
        ("anthropic", ModelTier::Flagship) => "claude-opus-4-6",
        ("gemini", ModelTier::Standard) => "gemini-2.0-flash",
        ("gemini", ModelTier::Flagship) => "gemini-2.5-pro",
        ("ollama", ModelTier::Standard) => "llama3.2",
        ("ollama", ModelTier::Flagship) => "llama3.1:70b",
        ("openrouter", ModelTier::Standard) => "anthropic/claude-sonnet-4-6",
        ("openrouter", ModelTier::Flagship) => "anthropic/claude-opus-4-6",
        ("groq", ModelTier::Standard) => "llama-3.3-70b-versatile",
        ("groq", ModelTier::Flagship) => "deepseek-r1-distill-llama-70b",
        ("mistral", ModelTier::Standard) => "mistral-small-latest",
        ("mistral", ModelTier::Flagship) => "mistral-large-latest",
        ("deepseek", ModelTier::Standard) => "deepseek-chat",
        ("deepseek", ModelTier::Flagship) => "deepseek-reasoner",
        ("fireworks", ModelTier::Standard) => "accounts/fireworks/models/llama-v3p3-70b-instruct",
        ("fireworks", ModelTier::Flagship) => "accounts/fireworks/models/deepseek-r1",
        ("xai", ModelTier::Standard) => "grok-3-mini",
        ("xai", ModelTier::Flagship) => "grok-3",
        ("bedrock", ModelTier::Standard) => "anthropic.claude-3-5-sonnet-20241022-v2:0",
        ("bedrock", ModelTier::Flagship) => "anthropic.claude-3-7-sonnet-20250219-v1:0",
        // claude-code: model is passed as a hint to the CLI; tier does not apply.
        _ => "",
    }
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
                let model = resolve_model("openai", config.model, config.tier)
                    .unwrap_or_else(|| "gpt-4o".to_string());
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "anthropic" => {
                let model = resolve_model("anthropic", config.model, config.tier)
                    .unwrap_or_else(|| "claude-3-7-sonnet-20250219".to_string());
                let p = crate::anthropic::AnthropicProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    model,
                    config.max_tokens.unwrap_or(8192),
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "gemini" => {
                let model = resolve_model("gemini", config.model, config.tier)
                    .unwrap_or_else(|| "gemini-2.5-flash".to_string());
                let p = crate::gemini::GeminiProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "ollama" => {
                let model = resolve_model("ollama", config.model, config.tier)
                    .unwrap_or_else(|| "llama3.2".to_string());
                let p = crate::ollama::OllamaProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "http://localhost:11434".to_string()),
                    model,
                    config.workspace_path,
                )?
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "openrouter" => {
                let model = resolve_model("openrouter", config.model, config.tier)
                    .unwrap_or_else(|| "anthropic/claude-3.5-sonnet".to_string());
                let p = crate::openrouter::OpenRouterProvider::from_config(
                    config.api_key.unwrap_or_default(),
                    model,
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
            // OpenAI-compatible third-party providers.
            "groq" => {
                let model = resolve_model("groq", config.model, config.tier)
                    .unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.groq.com/openai/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_name("groq")
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "mistral" => {
                let model = resolve_model("mistral", config.model, config.tier)
                    .unwrap_or_else(|| "mistral-small-latest".to_string());
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.mistral.ai/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_name("mistral")
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "deepseek" => {
                let model = resolve_model("deepseek", config.model, config.tier)
                    .unwrap_or_else(|| "deepseek-chat".to_string());
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_name("deepseek")
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "fireworks" => {
                let model =
                    resolve_model("fireworks", config.model, config.tier).unwrap_or_else(|| {
                        "accounts/fireworks/models/llama-v3p3-70b-instruct".to_string()
                    });
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.fireworks.ai/inference/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_name("fireworks")
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            "xai" => {
                let model = resolve_model("xai", config.model, config.tier)
                    .unwrap_or_else(|| "grok-3-mini".to_string());
                let p = crate::openai::OpenAiProvider::from_config(
                    config
                        .base_url
                        .unwrap_or_else(|| "https://api.x.ai/v1".to_string()),
                    config.api_key.unwrap_or_default(),
                    model,
                    config.workspace_path,
                )?
                .with_name("xai")
                .with_sandbox_profile(config.sandbox_profile.unwrap_or_default());
                Ok(Box::new(p))
            }
            #[cfg(feature = "bedrock")]
            "bedrock" => {
                let model = resolve_model("bedrock", config.model, config.tier)
                    .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string());
                let region = config.base_url.unwrap_or_else(|| "us-east-1".to_string());
                let access_key_id = config.api_key.unwrap_or_default();
                let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
                let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
                let p = crate::bedrock::BedrockProvider::from_config(
                    region,
                    access_key_id,
                    secret_access_key,
                    session_token,
                    model,
                    config.max_tokens.unwrap_or(8192),
                    config.workspace_path,
                )?
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
    ///
    /// Includes `"bedrock"` only when compiled with the `bedrock` feature.
    pub fn supported_providers() -> Vec<&'static str> {
        #[cfg(not(feature = "bedrock"))]
        return vec![
            "openai",
            "anthropic",
            "gemini",
            "ollama",
            "openrouter",
            "claude-code",
            "groq",
            "mistral",
            "deepseek",
            "fireworks",
            "xai",
        ];
        #[cfg(feature = "bedrock")]
        vec![
            "openai",
            "anthropic",
            "gemini",
            "ollama",
            "openrouter",
            "claude-code",
            "groq",
            "mistral",
            "deepseek",
            "fireworks",
            "xai",
            "bedrock",
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
            tier: None,
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
        assert!(providers.contains(&"groq"));
        assert!(providers.contains(&"mistral"));
        assert!(providers.contains(&"deepseek"));
        assert!(providers.contains(&"fireworks"));
        assert!(providers.contains(&"xai"));
        #[cfg(not(feature = "bedrock"))]
        assert_eq!(providers.len(), 11);
        #[cfg(feature = "bedrock")]
        assert_eq!(providers.len(), 12);
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

    #[test]
    fn model_from_tier_standard() {
        assert_eq!(
            model_from_tier("anthropic", ModelTier::Standard),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            model_from_tier("openai", ModelTier::Standard),
            "gpt-4o-mini"
        );
        assert_eq!(
            model_from_tier("gemini", ModelTier::Standard),
            "gemini-2.0-flash"
        );
        assert_eq!(model_from_tier("ollama", ModelTier::Standard), "llama3.2");
        assert_eq!(
            model_from_tier("openrouter", ModelTier::Standard),
            "anthropic/claude-sonnet-4-6"
        );
    }

    #[test]
    fn model_from_tier_flagship() {
        assert_eq!(
            model_from_tier("anthropic", ModelTier::Flagship),
            "claude-opus-4-6"
        );
        assert_eq!(model_from_tier("openai", ModelTier::Flagship), "gpt-4o");
        assert_eq!(
            model_from_tier("gemini", ModelTier::Flagship),
            "gemini-2.5-pro"
        );
        assert_eq!(
            model_from_tier("ollama", ModelTier::Flagship),
            "llama3.1:70b"
        );
        assert_eq!(
            model_from_tier("openrouter", ModelTier::Flagship),
            "anthropic/claude-opus-4-6"
        );
    }

    #[test]
    fn resolve_model_explicit_wins_over_tier() {
        let result = resolve_model(
            "anthropic",
            Some("my-custom-model".to_string()),
            Some(ModelTier::Flagship),
        );
        assert_eq!(result, Some("my-custom-model".to_string()));
    }

    #[test]
    fn resolve_model_tier_used_when_no_explicit_model() {
        let result = resolve_model("anthropic", None, Some(ModelTier::Standard));
        assert_eq!(result, Some("claude-sonnet-4-6".to_string()));
    }

    #[test]
    fn resolve_model_returns_none_when_both_absent() {
        let result = resolve_model("anthropic", None, None);
        assert!(result.is_none());
    }

    #[test]
    fn factory_creates_anthropic_with_tier() {
        let config = ProviderConfig {
            api_key: Some("sk-test".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            tier: Some(ModelTier::Standard),
            ..Default::default()
        };
        let result = ProviderFactory::create("anthropic", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "anthropic");
    }

    #[test]
    fn factory_creates_groq() {
        let config = ProviderConfig {
            api_key: Some("gsk_test".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("groq", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "groq");
    }

    #[test]
    fn factory_creates_mistral() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("mistral", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "mistral");
    }

    #[test]
    fn factory_creates_deepseek() {
        let config = ProviderConfig {
            api_key: Some("test-key".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("deepseek", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "deepseek");
    }

    #[test]
    fn factory_creates_fireworks() {
        let config = ProviderConfig {
            api_key: Some("fw_test".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("fireworks", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "fireworks");
    }

    #[test]
    fn factory_creates_xai() {
        let config = ProviderConfig {
            api_key: Some("xai_test".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("xai", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "xai");
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn factory_creates_bedrock() {
        let config = ProviderConfig {
            api_key: Some("AKIATEST".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("bedrock", config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "bedrock");
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn model_from_tier_bedrock() {
        assert_eq!(
            model_from_tier("bedrock", ModelTier::Standard),
            "anthropic.claude-3-5-sonnet-20241022-v2:0"
        );
        assert_eq!(
            model_from_tier("bedrock", ModelTier::Flagship),
            "anthropic.claude-3-7-sonnet-20250219-v1:0"
        );
    }

    #[test]
    fn factory_compat_providers_use_custom_base_url() {
        let config = ProviderConfig {
            api_key: Some("test".to_string()),
            base_url: Some("https://my-proxy.example.com/v1".to_string()),
            workspace_path: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let result = ProviderFactory::create("groq", config);
        assert!(result.is_ok());
    }

    #[test]
    fn model_from_tier_compat_providers() {
        assert_eq!(
            model_from_tier("groq", ModelTier::Standard),
            "llama-3.3-70b-versatile"
        );
        assert_eq!(
            model_from_tier("groq", ModelTier::Flagship),
            "deepseek-r1-distill-llama-70b"
        );
        assert_eq!(
            model_from_tier("mistral", ModelTier::Standard),
            "mistral-small-latest"
        );
        assert_eq!(
            model_from_tier("mistral", ModelTier::Flagship),
            "mistral-large-latest"
        );
        assert_eq!(
            model_from_tier("deepseek", ModelTier::Standard),
            "deepseek-chat"
        );
        assert_eq!(
            model_from_tier("deepseek", ModelTier::Flagship),
            "deepseek-reasoner"
        );
        assert_eq!(model_from_tier("xai", ModelTier::Standard), "grok-3-mini");
        assert_eq!(model_from_tier("xai", ModelTier::Flagship), "grok-3");
    }
}
