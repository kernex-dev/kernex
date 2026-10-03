//! Per-call OpenCode config, injected via `OPENCODE_CONFIG_CONTENT`.
//!
//! Permissions are always explicit `allow` or `deny`. A headless
//! `opencode run` has nobody to answer an `ask` prompt, so `ask` is never
//! emitted.

use kernex_core::context::McpServer;
use serde_json::{json, Map, Value};
use tracing::warn;

/// OpenCode built-in permission keys that kernex controls.
const PERMISSION_KEYS: &[&str] = &[
    "read",
    "edit",
    "glob",
    "grep",
    "list",
    "bash",
    "task",
    "external_directory",
    "todowrite",
    "webfetch",
    "websearch",
    "lsp",
    "skill",
];

/// Map a kernex/Claude Code tool name (or an OpenCode permission key) to
/// the OpenCode permission key that governs it.
fn permission_key(tool: &str) -> Option<&'static str> {
    let lower = tool.to_ascii_lowercase();
    let key = match lower.as_str() {
        "bash" => "bash",
        "read" => "read",
        "edit" | "write" | "multiedit" | "notebookedit" | "patch" => "edit",
        "glob" => "glob",
        "grep" => "grep",
        "ls" | "list" => "list",
        "task" => "task",
        "todowrite" | "todoread" => "todowrite",
        "webfetch" => "webfetch",
        "websearch" => "websearch",
        "lsp" => "lsp",
        "skill" => "skill",
        "external_directory" => "external_directory",
        _ => return None,
    };
    Some(key)
}

/// Build the permission map.
///
/// - `tools_disabled`: every tool denied (classification-style calls).
/// - `allowed_tools` empty: full access, every tool allowed.
/// - `allowed_tools` non-empty: listed tools allowed, the rest denied.
///
/// `question` (interactive) and `doom_loop` (asks for confirmation) are
/// always denied so a headless run can never block on a prompt.
pub(super) fn permissions(allowed_tools: &[String], tools_disabled: bool) -> Map<String, Value> {
    let mut map = Map::new();
    let full_access = !tools_disabled && allowed_tools.is_empty();
    let default = if full_access { "allow" } else { "deny" };
    for key in PERMISSION_KEYS {
        map.insert((*key).to_string(), json!(default));
    }
    if !tools_disabled {
        for tool in allowed_tools {
            match permission_key(tool) {
                Some(key) => {
                    map.insert(key.to_string(), json!("allow"));
                }
                None => warn!("opencode: no permission mapping for tool {tool:?}, ignoring"),
            }
        }
    }
    map.insert("question".to_string(), json!("deny"));
    map.insert("doom_loop".to_string(), json!("deny"));
    map
}

/// Build the `mcp` section from kernex's declared MCP servers.
fn mcp_section(servers: &[McpServer]) -> Map<String, Value> {
    let mut map = Map::new();
    for s in servers {
        let mut command = vec![Value::String(s.command.clone())];
        command.extend(s.args.iter().cloned().map(Value::String));
        map.insert(
            s.name.clone(),
            json!({
                "type": "local",
                "command": command,
                "environment": s.env,
                "enabled": true,
            }),
        );
    }
    map
}

/// Serialize the full per-call config.
pub(super) fn build_config(
    allowed_tools: &[String],
    tools_disabled: bool,
    mcp_servers: &[McpServer],
) -> String {
    let mut root = Map::new();
    root.insert(
        "permission".to_string(),
        Value::Object(permissions(allowed_tools, tools_disabled)),
    );
    if !mcp_servers.is_empty() {
        root.insert("mcp".to_string(), Value::Object(mcp_section(mcp_servers)));
    }
    Value::Object(root).to_string()
}
