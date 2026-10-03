//! CLI command building and subprocess execution.

use super::{OpenCodeProvider, ENV_PASSTHROUGH};
use crate::error::ProviderError;
use kernex_core::error::KernexError;
use tokio::process::Command;
use tracing::debug;

/// True if `s` would be parsed by the `opencode` CLI's argv parser as an
/// option rather than the value we intended. Context- or skill-supplied
/// values that start with `-` are dropped so they cannot smuggle a flag
/// into the subprocess.
fn looks_like_cli_flag(s: &str) -> bool {
    s.starts_with('-')
}

impl OpenCodeProvider {
    /// Build the CLI argument list (excluding the binary name).
    ///
    /// Pure so argument construction is testable without a subprocess. The
    /// prompt goes last, after `--`, so a message starting with `-` is never
    /// read as an option.
    pub(super) fn build_run_args(
        prompt: &str,
        model: &str,
        session_id: Option<&str>,
        agent_name: Option<&str>,
    ) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];

        // Agent names select an OpenCode agent definition by name. Reject
        // path separators, traversal and a leading `-`.
        let agent = agent_name
            .filter(|n| !n.is_empty())
            .filter(|n| !n.contains('/') && !n.contains('\\') && !n.contains(".."))
            .filter(|n| !looks_like_cli_flag(n));
        if let Some(name) = agent {
            args.push("--agent".to_string());
            args.push(name.to_string());
        }

        if !model.is_empty() && !looks_like_cli_flag(model) {
            args.push("--model".to_string());
            args.push(model.to_string());
        }

        if let Some(sid) = session_id.filter(|s| !s.is_empty() && !looks_like_cli_flag(s)) {
            args.push("--session".to_string());
            args.push(sid.to_string());
        }

        args.push("--".to_string());
        args.push(prompt.to_string());
        args
    }

    /// Run `opencode run` with the per-call config and a timeout.
    pub(super) async fn run_cli(
        &self,
        args: &[String],
        config_json: &str,
    ) -> Result<std::process::Output, KernexError> {
        let mut cmd = self.base_command()?;
        cmd.env("OPENCODE_CONFIG_CONTENT", config_json);
        cmd.args(args);
        debug!("executing: opencode run --format json <prompt>");

        let output = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| {
                ProviderError::Logic(format!(
                    "opencode CLI timed out after {}s",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|e| ProviderError::Logic(format!("failed to run opencode CLI: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ProviderError::Logic(format!(
                "opencode CLI exited with {}: {stderr}",
                output.status
            ))
            .into());
        }
        Ok(output)
    }

    /// Build the base `Command` with working directory and system protection.
    ///
    /// Same posture as the `claude-code` provider: sandboxed when a working
    /// directory is configured, and an error when OS-level enforcement is
    /// required but would not apply.
    fn base_command(&self) -> Result<Command, KernexError> {
        let mut cmd = match self.working_dir {
            Some(ref dir) => {
                // Writes to the data dir (parent of the workspace) are
                // blocked, so memory.db stays out of the agent's reach.
                let data_dir = dir.parent().unwrap_or(dir);
                // OpenCode IS the provider: it must reach the model API, so
                // it is exempt from the subprocess egress deny-by-default.
                let mut profile = self.sandbox_profile.clone();
                profile.allow_network = true;
                let mut c = kernex_sandbox::try_protected_command("opencode", data_dir, &profile)
                    .map_err(|e| {
                    ProviderError::Logic(format!("refusing to run opencode CLI: {e}"))
                })?;
                c.current_dir(dir);
                c
            }
            None => {
                if kernex_sandbox::enforcement_required(&self.sandbox_profile) {
                    return Err(ProviderError::Logic(
                        "refusing to run opencode CLI: OS-level sandbox enforcement is \
                         required but no working directory is configured, so the \
                         subprocess would run unsandboxed"
                            .to_string(),
                    )
                    .into());
                }
                let mut c = Command::new("opencode");
                kernex_sandbox::hardened_env(&mut c);
                c
            }
        };

        // The hardened env cleared everything; pass the model provider
        // credentials and OpenCode's own location variables back in.
        let extra = self.extra_env.iter().map(String::as_str);
        for name in ENV_PASSTHROUGH.iter().copied().chain(extra) {
            if let Ok(value) = std::env::var(name) {
                if !value.is_empty() {
                    cmd.env(name, value);
                }
            }
        }
        Ok(cmd)
    }
}
