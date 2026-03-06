//! MCP server settings management for Claude Code CLI.
//!
//! Handles writing and cleaning up `.claude/settings.local.json` files
//! that configure MCP servers for the CLI subprocess.

use kernex_core::{context::McpServer, error::KernexError};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Write `.claude/settings.local.json` with MCP server configuration.
///
/// Claude Code reads this file from `current_dir` on startup.
pub(super) async fn write_mcp_settings(
    workspace: &Path,
    servers: &[McpServer],
) -> Result<PathBuf, KernexError> {
    let claude_dir = workspace.join(".claude");
    tokio::fs::create_dir_all(&claude_dir)
        .await
        .map_err(|e| KernexError::Provider(format!("failed to create .claude dir: {e}")))?;

    let path = claude_dir.join("settings.local.json");

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
        .map_err(|e| KernexError::Provider(format!("failed to serialize MCP settings: {e}")))?;

    tokio::fs::write(&path, json)
        .await
        .map_err(|e| KernexError::Provider(format!("failed to write MCP settings: {e}")))?;

    info!("mcp: wrote settings to {}", path.display());
    Ok(path)
}

/// Remove the temporary MCP settings file.
pub(super) async fn cleanup_mcp_settings(path: &Path) {
    if path.exists() {
        if let Err(e) = tokio::fs::remove_file(path).await {
            warn!("mcp: failed to cleanup {}: {e}", path.display());
        } else {
            debug!("mcp: cleaned up {}", path.display());
        }
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
