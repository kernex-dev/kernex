//! Provider trait implementation.

use super::{config, events, OpenCodeProvider};
use crate::cli_support::{self, CliWorkspace};
use crate::error::ProviderError;
use async_trait::async_trait;
use kernex_core::{
    error::KernexError,
    message::{CompletionMeta, Response},
    traits::Provider,
};
use std::time::Instant;

#[async_trait]
impl Provider for OpenCodeProvider {
    fn name(&self) -> &str {
        "opencode"
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn complete(
        &self,
        context: &kernex_core::context::Context,
    ) -> Result<Response, KernexError> {
        let prompt = context.to_prompt_string();
        let start = Instant::now();

        let model = context.model.as_deref().unwrap_or(&self.model);
        // Without a configured working directory the call gets an empty
        // temporary one, so OpenCode always runs inside the sandbox.
        let workspace = CliWorkspace::for_call(self.working_dir.as_deref()).map_err(|e| {
            ProviderError::Logic(format!("cannot create a workspace for opencode CLI: {e}"))
        })?;
        let allowed = context.allowed_tools.as_deref();
        let servers = cli_support::servers_for_cli(
            &context.mcp_servers,
            allowed,
            workspace.path(),
            "opencode",
        );
        let config_json =
            config::build_config(allowed, self.permission_bypass, &self.web_fetch, &servers);
        let args = Self::build_run_args(
            &prompt,
            model,
            context.session_id.as_deref(),
            context.agent_name.as_deref(),
        );

        let output = self
            .run_cli(&args, &config_json, workspace.path(), model)
            .await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let run = events::parse_events(&stdout);

        // OpenCode can exit 0 after a failed run; the error event and an
        // empty reply are the signals.
        if run.text.is_empty() {
            let reason = run
                .error
                .unwrap_or_else(|| "no text in the event stream".to_string());
            return Err(ProviderError::Logic(format!("opencode run failed: {reason}")).into());
        }

        Ok(Response {
            text: run.text,
            metadata: CompletionMeta {
                provider_used: "opencode".to_string(),
                tokens_used: run.tokens_used,
                processing_time_ms: start.elapsed().as_millis() as u64,
                model: (!model.is_empty()).then(|| model.to_string()),
                stop_reason: run.stop_reason,
                session_id: run.session_id,
                ..Default::default()
            },
        })
    }

    async fn is_available(&self) -> bool {
        Self::check_cli().await
    }
}
