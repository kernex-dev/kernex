//! MCP server config for the Claude Code CLI.
//!
//! Writes the declared MCP servers to a throwaway temp file and passes it to
//! the CLI via `--mcp-config <file>`. This is Claude Code's own mechanism for
//! loading MCP servers from an arbitrary JSON file, so kernex never writes to
//! (or deletes) the user's `.claude/settings.local.json` — which Claude Code
//! does not read for MCP and which holds the user's own permissions/hooks.

use crate::error::ProviderError;
use kernex_core::{context::McpServer, error::KernexError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Serialize the declared MCP servers to a throwaway temp file (the
/// `{"mcpServers": {...}}` shape Claude Code's `--mcp-config` expects) and
/// return its path. The caller passes it via `--mcp-config` and removes it
/// with [`cleanup_mcp_config`] once the CLI calls finish.
pub(super) async fn write_mcp_config_tempfile(
    servers: &[McpServer],
) -> Result<PathBuf, KernexError> {
    let mut mcp_servers = serde_json::Map::new();
    for srv in servers {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "command".to_string(),
            serde_json::Value::String(srv.command.clone()),
        );
        entry.insert(
            "args".to_string(),
            serde_json::Value::Array(
                srv.args
                    .iter()
                    .map(|a| serde_json::Value::String(a.clone()))
                    .collect(),
            ),
        );
        if !srv.env.is_empty() {
            let env_obj: serde_json::Map<String, serde_json::Value> = srv
                .env
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            entry.insert("env".to_string(), serde_json::Value::Object(env_obj));
        }
        mcp_servers.insert(srv.name.clone(), serde_json::Value::Object(entry));
    }

    let mut root = serde_json::Map::new();
    root.insert(
        "mcpServers".to_string(),
        serde_json::Value::Object(mcp_servers),
    );

    let json = serde_json::to_string_pretty(&root)
        .map_err(|e| ProviderError::Logic(format!("failed to serialize MCP config: {e}")))?;

    // A uniquely-named file in the system temp dir (pid + a per-process
    // counter), outside the user's config tree. The CLI subprocess reads it via
    // `--mcp-config`; we delete it ourselves in `cleanup_mcp_config`.
    //
    // Created with O_EXCL so a pre-existing path (a planted symlink on a
    // shared machine, or a stale leftover) is never followed or overwritten -
    // on collision we skip to the next counter value. Mode 0600 keeps any
    // configured MCP env values (potential credentials) unreadable by other
    // users regardless of umask.
    static MCP_CONFIG_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut last_err: Option<std::io::Error> = None;
    for _ in 0..8 {
        let seq = MCP_CONFIG_SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kernex-mcp-{}-{seq}.json", std::process::id()));
        let bytes = json.clone();
        let write_path = path.clone();
        let res = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&write_path)?;
            f.write_all(bytes.as_bytes())
        })
        .await
        .map_err(|e| ProviderError::Logic(format!("failed to write MCP config: {e}")))?;

        match res {
            Ok(()) => {
                debug!("mcp: wrote config to {}", path.display());
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Never reuse or follow a path we did not create.
                last_err = Some(e);
                continue;
            }
            Err(e) => {
                return Err(
                    ProviderError::Logic(format!("failed to write MCP config: {e}")).into(),
                );
            }
        }
    }
    Err(ProviderError::Logic(format!(
        "failed to create a unique MCP config after 8 attempts: {last_err:?}"
    ))
    .into())
}

/// Remove the temporary MCP config file written by [`write_mcp_config_tempfile`].
pub(super) async fn cleanup_mcp_config(path: &Path) {
    // Unconditional remove: an exists() pre-check races temp cleaners into a
    // spurious warning. NotFound just means the job is already done.
    match tokio::fs::remove_file(path).await {
        Ok(()) => debug!("mcp: cleaned up {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!("mcp: failed to cleanup {}: {e}", path.display()),
    }
}

/// Generate `--allowedTools` patterns for MCP servers.
///
/// Each server gets a `mcp__<name>__*` wildcard pattern.
pub fn mcp_tool_patterns(servers: &[McpServer]) -> Vec<String> {
    servers
        .iter()
        .map(|s| format!("mcp__{}__*", s.name))
        .collect()
}
