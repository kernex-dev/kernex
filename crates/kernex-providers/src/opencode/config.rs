//! Per-call OpenCode config, injected via `OPENCODE_CONFIG_CONTENT`.
//!
//! Permissions are always explicit `allow` or `deny`. A headless
//! `opencode run` has nobody to answer an `ask` prompt, so `ask` is never
//! emitted.

use kernex_core::context::McpServer;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::WebFetchPolicy;

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

/// Keys allowed when no tool list applies and the permission bypass is off:
/// the tools that read and change nothing outside OpenCode's own state.
const READ_ONLY_KEYS: &[&str] = &["read", "glob", "grep", "list", "lsp", "todowrite"];

/// Map a kernex/Claude Code tool name (or an OpenCode permission key) to
/// the OpenCode permission key that governs it.
fn permission_key(tool: &str) -> Option<&'static str> {
    let name = tool.split('(').next().unwrap_or(tool).trim();
    let folded: String = name
        .chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let key = match folded.as_str() {
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
        "codesearch" => "codesearch",
        "lsp" => "lsp",
        "skill" => "skill",
        "externaldirectory" => "external_directory",
        _ => return None,
    };
    Some(key)
}

/// OpenCode tools that reach the network on the model's behalf.
const WEB_KEYS: &[&str] = &["webfetch", "websearch", "codesearch"];

/// OpenCode's spelling of a name inside an MCP tool name.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The permission patterns (OpenCode names MCP tools `<server>_<tool>`)
/// that admit the passed servers' tools under `allowed`.
fn mcp_patterns(allowed: Option<&[String]>, servers: &[McpServer]) -> Vec<String> {
    let mut out = Vec::new();
    for server in servers {
        let name = sanitize(&server.name);
        let Some(list) = allowed else {
            out.push(format!("{name}_*"));
            continue;
        };
        for entry in list {
            let entry = entry.split('(').next().unwrap_or(entry).trim();
            let Some(rest) = entry.strip_prefix("mcp__") else {
                continue;
            };
            let (owner, tool) = match rest.split_once("__") {
                Some((owner, tool)) => (owner, Some(tool)),
                None => (rest, None),
            };
            if owner != server.name {
                continue;
            }
            let pattern = match tool {
                Some(tool) if tool != "*" => format!("{name}_{}", sanitize(tool)),
                _ => format!("{name}_*"),
            };
            if !out.contains(&pattern) {
                out.push(pattern);
            }
        }
    }
    out
}

/// Build the permission map.
///
/// The first rule is `"*"`: OpenCode's own default allows every tool it has
/// (and the MCP servers of its own config), so anything kernex does not
/// name must be denied explicitly. OpenCode takes the last matching rule,
/// so the specific keys after it win.
///
/// - `allowed` is `None`: with `bypass` everything is allowed; without it
///   only the read-only keys and the passed MCP servers' tools are.
/// - `allowed` is a list: the keys and MCP tools it names are allowed, the
///   rest denied (an empty list denies everything). `bypass` does not apply.
///
/// The `web_fetch` policy then applies to the web tools: OpenCode has no
/// per-host rule, so anything short of `AllowAll` denies them.
/// `question` (interactive) and `doom_loop` (asks for confirmation) are
/// always denied so a headless run can never block on a prompt.
pub(super) fn permissions(
    allowed: Option<&[String]>,
    bypass: bool,
    web_fetch: &WebFetchPolicy,
    servers: &[McpServer],
) -> Map<String, Value> {
    let mut map = Map::new();
    let all = allowed.is_none() && bypass;
    let default = json!(if all { "allow" } else { "deny" });
    map.insert("*".to_string(), default.clone());
    for key in PERMISSION_KEYS.iter().chain(WEB_KEYS) {
        map.insert((*key).to_string(), default.clone());
    }
    match allowed {
        None if !bypass => {
            for key in READ_ONLY_KEYS {
                map.insert((*key).to_string(), json!("allow"));
            }
        }
        None => {}
        Some(list) => {
            for tool in list.iter().filter(|t| !t.starts_with("mcp__")) {
                match permission_key(tool) {
                    Some(key) => {
                        map.insert(key.to_string(), json!("allow"));
                    }
                    None => warn!("opencode: no permission mapping for tool {tool:?}, ignoring"),
                }
            }
        }
    }
    if !all {
        for pattern in mcp_patterns(allowed, servers) {
            map.insert(pattern, json!("allow"));
        }
    }
    if !matches!(web_fetch, WebFetchPolicy::AllowAll) {
        if matches!(web_fetch, WebFetchPolicy::AllowHosts(_))
            && WEB_KEYS.iter().any(|k| map[*k] == "allow")
        {
            warn!("opencode: web tools are turned off: OpenCode has no per-host fetch rule");
        }
        for key in WEB_KEYS {
            map.insert((*key).to_string(), json!("deny"));
        }
    }
    map.insert("question".to_string(), json!("deny"));
    map.insert("doom_loop".to_string(), json!("deny"));
    map
}

/// Build the `mcp` section from servers already chosen for the CLI.
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
    allowed: Option<&[String]>,
    bypass: bool,
    web_fetch: &WebFetchPolicy,
    mcp_servers: &[McpServer],
) -> String {
    let mut root = Map::new();
    root.insert(
        "permission".to_string(),
        Value::Object(permissions(allowed, bypass, web_fetch, mcp_servers)),
    );
    if !mcp_servers.is_empty() {
        root.insert("mcp".to_string(), Value::Object(mcp_section(mcp_servers)));
    }
    Value::Object(root).to_string()
}
