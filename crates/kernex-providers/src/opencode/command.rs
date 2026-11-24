//! CLI command building and subprocess execution.

use super::{provider_key_env, OpenCodeProvider, XDG_DIRS};
use crate::error::ProviderError;
use kernex_core::error::KernexError;
use std::path::Path;
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

    /// Run `opencode run` in `workspace` with the per-call config and a
    /// timeout. `model` selects which provider key is passed through.
    pub(super) async fn run_cli(
        &self,
        args: &[String],
        config_json: &str,
        workspace: &Path,
        model: &str,
    ) -> Result<std::process::Output, KernexError> {
        let (mut cmd, _cache) = self.base_command(workspace, model)?;
        cmd.env("OPENCODE_CONFIG_CONTENT", config_json);
        cmd.args(args);
        debug!("executing: opencode run --format json <prompt>");

        let output = crate::cli_support::run_with_timeout(cmd, self.timeout)
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

    /// Build the base `Command`: OpenCode sandboxed in `workspace`, with its
    /// own directories, network access and the key of the model provider
    /// `model` names. The returned directory is this call's cache; it is
    /// removed when dropped, after the call.
    ///
    /// Errors when OS-level sandbox enforcement is required (profile flag or
    /// `KERNEX_REQUIRE_SANDBOX=1`) and the host cannot provide it.
    fn base_command(
        &self,
        workspace: &Path,
        model: &str,
    ) -> Result<(Command, tempfile::TempDir), KernexError> {
        let state = self.cli_state_dir();
        let refuse = |e: std::io::Error| {
            ProviderError::Logic(format!(
                "refusing to run opencode CLI: cannot prepare {}: {e}",
                state.display()
            ))
        };
        for (_, sub) in XDG_DIRS {
            crate::cli_support::create_private_dir(&state.join(sub)).map_err(refuse)?;
        }
        // OpenCode's tools run in OpenCode's own sandbox and can write what it
        // writes. So that nothing one call leaves there steers the next:
        // - its config directory is read-only (no agent, plugin or command
        //   can be added);
        // - its login files are removed before every call (logins come from
        //   API key variables only, and a planted entry could make OpenCode
        //   fetch and merge a remote config);
        // - its cache, where it installs plugin packages and skills, is new
        //   for every call.
        for file in ["auth.json", "mcp-auth.json"] {
            let path = state.join("data").join("opencode").join(file);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(refuse(e).into()),
            }
        }
        let cache = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(state.join("cache"))
            .map_err(refuse)?;
        let scope = self.sandbox_dirs.scope(workspace);
        // OpenCode IS the provider: it must reach the model API, so it is
        // exempt from the subprocess egress deny-by-default.
        let mut profile = self.sandbox_profile.clone();
        profile.allow_network = true;
        for sub in ["data", "state", "cache"] {
            profile.allowed_paths.push(state.join(sub));
        }
        let mut cmd = kernex_sandbox::try_protected_command(&self.program, &scope, &profile)
            .map_err(|e| ProviderError::Logic(format!("refusing to run opencode CLI: {e}")))?;
        cmd.current_dir(workspace);
        for (var, sub) in XDG_DIRS {
            cmd.env(var, state.join(sub));
        }
        cmd.env("XDG_CACHE_HOME", cache.path());
        // Neither `opencode.json` nor `.opencode/` from the workspace, which
        // the model can write.
        cmd.env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");

        let keys = provider_key_env(model);
        if keys.is_empty() && self.extra_env.is_empty() {
            debug!("opencode: no provider key passed for model {model:?}");
        }
        // The hardened env cleared everything; pass back the configured
        // provider's key and the host's extras.
        let names = keys
            .iter()
            .copied()
            .chain(self.extra_env.iter().map(String::as_str));
        for name in names {
            if let Some(value) = (self.env_source)(name) {
                if !value.is_empty() {
                    cmd.env(name, value);
                }
            }
        }
        Ok((cmd, cache))
    }
}
