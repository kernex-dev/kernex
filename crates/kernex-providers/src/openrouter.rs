//! OpenRouter proxy provider.
//!
//! Reuses OpenAI's request/response types and agentic loop.
//! Only the base URL and provider name differ.

use async_trait::async_trait;
use kernex_core::{context::Context, error::KernexError, message::Response, traits::Provider};
use secrecy::{ExposeSecret, SecretString};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, warn};

use crate::http_retry::send_with_retry;
use crate::openai::{
    build_openai_messages, openai_agentic_complete, ChatCompletionRequest, ChatCompletionResponse,
};
use crate::tools::{build_response, tools_enabled, ToolExecutor};

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Default max agentic loop iterations.
const DEFAULT_MAX_TURNS: u32 = 50;

/// OpenRouter provider — routes requests to many models via the OpenAI-compatible API.
pub struct OpenRouterProvider {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    workspace_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
}

impl OpenRouterProvider {
    /// Create from config values.
    pub fn from_config(
        api_key: String,
        model: String,
        workspace_path: Option<PathBuf>,
    ) -> Result<Self, KernexError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| KernexError::Provider(format!("failed to build HTTP client: {e}")))?,
            api_key: SecretString::new(api_key),
            model,
            workspace_path,
            sandbox_profile: Default::default(),
        })
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        let (system, api_messages) = context.to_api_messages();
        let effective_model = context.model.as_deref().unwrap_or(&self.model);
        let url = format!("{OPENROUTER_BASE_URL}/chat/completions");
        let auth = format!("Bearer {}", self.api_key.expose_secret());
        let max_turns = context.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        let has_tools = tools_enabled(context);

        if has_tools {
            if let Some(ref ws) = self.workspace_path {
                let mut executor = ToolExecutor::new(ws.clone())
                    .with_sandbox_profile(self.sandbox_profile.clone())
                    .with_hook_runner_opt(context.hook_runner.clone());
                executor.connect_mcp_servers(&context.mcp_servers).await;
                executor.register_toolboxes(&context.toolboxes);

                let result = openai_agentic_complete(
                    &self.client,
                    &url,
                    &auth,
                    effective_model,
                    &system,
                    &api_messages,
                    &mut executor,
                    max_turns,
                    "openrouter",
                )
                .await;

                executor.shutdown_mcp().await;
                return result;
            }
        }

        // Fallback: no tools.
        let start = Instant::now();
        let messages = build_openai_messages(&system, &api_messages);
        let body = ChatCompletionRequest {
            model: effective_model.to_string(),
            messages,
            tools: None,
        };

        debug!("openrouter: POST {url} model={effective_model} (no tools)");

        let body_json = serde_json::to_vec(&body)
            .map_err(|e| KernexError::Provider(format!("openrouter: serialize failed: {e}")))?;

        let resp = {
            let client = &self.client;
            let url = &url;
            let auth = &auth;
            send_with_retry("openrouter", || {
                let req = client
                    .post(url.as_str())
                    .header("Authorization", auth.as_str())
                    .header("Content-Type", "application/json")
                    .body(body_json.clone());
                async move { req.send().await }
            })
            .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(KernexError::Provider(format!(
                "openrouter returned {status}: {text}"
            )));
        }

        let parsed: ChatCompletionResponse = resp.json().await.map_err(|e| {
            KernexError::Provider(format!("openrouter: failed to parse response: {e}"))
        })?;

        let text = parsed
            .choices
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| "No response from OpenRouter.".to_string());

        let tokens = parsed
            .usage
            .as_ref()
            .and_then(|u| u.total_tokens)
            .unwrap_or(0);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(build_response(
            text,
            "openrouter",
            tokens,
            elapsed_ms,
            parsed.model,
        ))
    }

    async fn is_available(&self) -> bool {
        if self.api_key.expose_secret().is_empty() {
            warn!("openrouter: no API key configured");
            return false;
        }
        let url = format!("{OPENROUTER_BASE_URL}/models");
        match self
            .client
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("openrouter not available: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openrouter_provider_name() {
        let p = OpenRouterProvider::from_config(
            "sk-or-test".into(),
            "anthropic/claude-sonnet-4".into(),
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "openrouter");
        assert!(p.requires_api_key());
    }

    #[test]
    fn test_openrouter_base_url() {
        assert_eq!(OPENROUTER_BASE_URL, "https://openrouter.ai/api/v1");
    }
}
