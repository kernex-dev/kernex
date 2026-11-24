//! Shared tool executor for HTTP-based providers.
//!
//! Provides 7 built-in tools (Bash, Read, Write, Edit, Grep, Glob, WebFetch)
//! with sandbox enforcement, plus MCP server tool routing. Used by all agentic loops.

use crate::mcp_client::McpClient;
use kernex_core::context::{McpServer, Toolbox};
use kernex_core::message::{CompletionMeta, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

mod files;
mod gates;
mod web_fetch;

#[cfg(test)]
use web_fetch::is_safe_fetch_url;
pub use web_fetch::WebFetchPolicy;

/// Maximum characters for bash tool output before truncation.
const MAX_BASH_OUTPUT: usize = 30_000;
/// Maximum characters for read tool output before truncation.
const MAX_READ_OUTPUT: usize = 50_000;
/// Default bash command timeout in seconds.
const BASH_TIMEOUT_SECS: u64 = 120;
/// Default toolbox script timeout in seconds.
const TOOLBOX_TIMEOUT_SECS: u64 = 120;
/// Maximum length of a `grep` pattern. Bounds ReDoS attack surface:
/// pathological backtracking regexes hit this length cap before the
/// 30 s timeout becomes the only line of defence.
const MAX_GREP_PATTERN_LEN: usize = 1024;

/// A tool definition in provider-agnostic format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters: Value,
}

/// Result of executing a tool.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Text output from the tool.
    pub content: String,
    /// Whether the tool call failed.
    pub is_error: bool,
}

/// Executes built-in tools, script-based toolboxes, and MCP tool calls.
pub struct ToolExecutor {
    workspace_path: PathBuf,
    scope: kernex_sandbox::SandboxScope,
    sandbox_profile: kernex_sandbox::SandboxProfile,
    mcp_clients: HashMap<String, McpClient>,
    mcp_tool_map: HashMap<String, String>,
    toolboxes: HashMap<String, Toolbox>,
    hook_runner: Option<std::sync::Arc<dyn kernex_core::hooks::HookRunner>>,
    permission_rules: Option<std::sync::Arc<kernex_core::permissions::PermissionRules>>,
    web_fetch: WebFetchPolicy,
    /// `None` offers every tool, `Some(list)` exactly the tools it names.
    allowed_tools: Option<Vec<String>>,
    /// Lets a test fetch from a server on loopback over plain http.
    #[cfg(test)]
    fetch_allows_local: bool,
}

impl ToolExecutor {
    /// Create a new tool executor.
    ///
    /// `workspace_path` is the working directory and the only directory in
    /// the sandbox scope that tools may write. The runtime data directory
    /// defaults to the runtime's default location until
    /// [`with_sandbox_dirs`](Self::with_sandbox_dirs) names the real one.
    pub fn new(workspace_path: PathBuf) -> Self {
        let scope = crate::SandboxDirs::default().build_scope(&workspace_path);
        Self {
            workspace_path,
            scope,
            sandbox_profile: Default::default(),
            mcp_clients: HashMap::new(),
            mcp_tool_map: HashMap::new(),
            toolboxes: HashMap::new(),
            hook_runner: None,
            permission_rules: None,
            web_fetch: WebFetchPolicy::default(),
            allowed_tools: None,
            #[cfg(test)]
            fetch_allows_local: false,
        }
    }

    /// Set the runtime directories the sandbox protects.
    ///
    /// Tool calls cannot write the data directory's database, skills,
    /// projects or configuration, and cannot read the database or the
    /// configuration file, wherever the host keeps them.
    pub fn with_sandbox_dirs(mut self, dirs: &crate::SandboxDirs) -> Self {
        self.scope = dirs.scope(&self.workspace_path);
        self
    }

    /// Set which hosts the `web_fetch` tool may reach. The default is
    /// [`WebFetchPolicy::Disabled`]: the tool is not offered to the model.
    pub fn with_web_fetch_policy(mut self, policy: WebFetchPolicy) -> Self {
        self.web_fetch = policy;
        self
    }

    /// Restrict the executor to the tools `allowed` names: built-in,
    /// toolbox and MCP tools alike, compared without regard to case.
    /// `None` (the default) allows every tool and `Some(vec![])` none. A
    /// tool that is not allowed is neither offered to the model nor run.
    pub fn with_allowed_tools(mut self, allowed: Option<Vec<String>>) -> Self {
        self.allowed_tools = allowed;
        self
    }

    /// Set a custom sandbox profile.
    pub fn with_sandbox_profile(mut self, profile: kernex_sandbox::SandboxProfile) -> Self {
        self.sandbox_profile = profile;
        self
    }

    /// Attach a hook runner for pre/post tool lifecycle events.
    #[allow(dead_code)]
    pub fn with_hook_runner(
        mut self,
        runner: std::sync::Arc<dyn kernex_core::hooks::HookRunner>,
    ) -> Self {
        self.hook_runner = Some(runner);
        self
    }

    /// Optionally attach a hook runner (no-op when `None`).
    pub fn with_hook_runner_opt(
        mut self,
        runner: Option<std::sync::Arc<dyn kernex_core::hooks::HookRunner>>,
    ) -> Self {
        self.hook_runner = runner;
        self
    }

    /// Optionally attach declarative permission rules (no-op when `None`).
    pub fn with_permission_rules_opt(
        mut self,
        rules: Option<std::sync::Arc<kernex_core::permissions::PermissionRules>>,
    ) -> Self {
        self.permission_rules = rules;
        self
    }

    /// Register script-based toolbox tools.
    pub fn register_toolboxes(&mut self, toolboxes: &[Toolbox]) {
        for tb in toolboxes {
            debug!("toolbox: registered '{}'", tb.name);
            self.toolboxes.insert(tb.name.clone(), tb.clone());
        }
    }

    /// Connect to MCP servers and discover their tools.
    pub async fn connect_mcp_servers(&mut self, servers: &[McpServer]) {
        for server in servers {
            match McpClient::connect(&server.name, &server.command, &server.args, &server.env).await
            {
                Ok(client) => {
                    // Map each tool name to this server.
                    for tool in &client.tools {
                        self.mcp_tool_map
                            .insert(tool.name.clone(), server.name.clone());
                    }
                    self.mcp_clients.insert(server.name.clone(), client);
                }
                Err(e) => {
                    warn!("mcp: failed to connect to '{}': {e}", server.name);
                }
            }
        }
    }

    /// Return all available tool definitions (built-in + toolbox + MCP).
    ///
    /// `web_fetch` is left out while its policy is
    /// [`WebFetchPolicy::Disabled`].
    pub fn all_tool_defs(&self) -> Vec<ToolDef> {
        let mut defs = builtin_tool_defs();

        // Add toolbox tools.
        for tb in self.toolboxes.values() {
            defs.push(ToolDef {
                name: tb.name.clone(),
                description: tb.description.clone(),
                parameters: tb.parameters.clone(),
            });
        }

        // Add MCP tools — sorted by server name for deterministic ordering
        // (avoids prompt cache misses caused by HashMap non-determinism).
        let mut mcp_server_names: Vec<&str> = self.mcp_clients.keys().map(|s| s.as_str()).collect();
        mcp_server_names.sort_unstable();
        for server_name in mcp_server_names {
            if let Some(client) = self.mcp_clients.get(server_name) {
                for mcp_tool in &client.tools {
                    defs.push(ToolDef {
                        name: mcp_tool.name.clone(),
                        description: mcp_tool.description.clone(),
                        parameters: mcp_tool.input_schema.clone(),
                    });
                }
            }
        }

        // The name always routes to the built-in tool, so a toolbox or MCP
        // tool that shares it is not offered either.
        if !self.web_fetch.is_enabled() {
            defs.retain(|def| def.name != "web_fetch");
        }
        defs.retain(|def| self.tool_allowed(&def.name));
        defs
    }

    /// Execute a tool call by name, routing to built-in or MCP.
    ///
    /// Fires pre/post hooks when a [`HookRunner`] is attached. A blocked
    /// pre-hook short-circuits execution and returns an error result.
    ///
    /// [`HookRunner`]: kernex_core::hooks::HookRunner
    pub async fn execute(&mut self, tool_name: &str, args: &Value) -> ToolResult {
        // The allowlist comes first: a tool the host did not allow reaches
        // neither the hooks nor the rules.
        if !self.tool_allowed(tool_name) {
            return ToolResult {
                content: format!("Tool '{tool_name}' is not allowed in this session"),
                is_error: true,
            };
        }

        // Pre-tool hook.
        if let Some(runner) = &self.hook_runner.clone() {
            match runner.pre_tool(tool_name, args).await {
                kernex_core::hooks::HookOutcome::Allow => {}
                kernex_core::hooks::HookOutcome::Blocked(reason) => {
                    return ToolResult {
                        content: format!("Tool blocked by hook: {reason}"),
                        is_error: true,
                    };
                }
            }
        }

        // Permission rules check.
        if let Some(rules) = &self.permission_rules {
            let owned = self.rule_subjects(tool_name, args);
            let subjects: Vec<&str> = owned.iter().map(String::as_str).collect();
            if let kernex_core::permissions::PermissionOutcome::Deny(reason) =
                rules.check_subjects(tool_name, &subjects)
            {
                return ToolResult {
                    content: format!("Tool call denied: {reason}"),
                    is_error: true,
                };
            }
        }

        let result = self.dispatch(tool_name, args).await;

        // Post-tool hook.
        if let Some(runner) = &self.hook_runner.clone() {
            runner
                .post_tool(tool_name, &result.content, result.is_error)
                .await;
        }

        result
    }

    /// Route a tool call to the appropriate implementation.
    async fn dispatch(&mut self, tool_name: &str, args: &Value) -> ToolResult {
        match tool_name.to_lowercase().as_str() {
            "bash" => self.exec_bash(args).await,
            "read" => self.exec_read(args).await,
            "write" => self.exec_write(args).await,
            "edit" => self.exec_edit(args).await,
            "grep" => self.exec_grep(args).await,
            "glob" => self.exec_glob(args).await,
            "web_fetch" => self.exec_web_fetch(args).await,
            _ => {
                // Try toolbox routing.
                if let Some(tb) = self.toolboxes.get(tool_name).cloned() {
                    return self.exec_toolbox(&tb, args).await;
                }

                // Try MCP routing.
                if let Some(server_name) = self.mcp_tool_map.get(tool_name).cloned() {
                    if let Some(client) = self.mcp_clients.get_mut(&server_name) {
                        match client.call_tool(tool_name, args).await {
                            Ok(r) => ToolResult {
                                content: r.content,
                                is_error: r.is_error,
                            },
                            Err(e) => ToolResult {
                                content: format!("MCP error: {e}"),
                                is_error: true,
                            },
                        }
                    } else {
                        ToolResult {
                            content: format!("MCP server '{server_name}' not connected"),
                            is_error: true,
                        }
                    }
                } else {
                    ToolResult {
                        content: format!("Unknown tool: {tool_name}"),
                        is_error: true,
                    }
                }
            }
        }
    }

    /// Shut down all MCP server connections.
    pub async fn shutdown_mcp(&mut self) {
        for (name, client) in self.mcp_clients.drain() {
            debug!("mcp: shutting down '{name}'");
            client.shutdown().await;
        }
        self.mcp_tool_map.clear();
    }

    /// Resolve a path string to a normalized absolute path.
    ///
    /// Relative paths are joined against `workspace_path` and then
    /// lexically normalized (removing `.` and `..` components) to
    /// prevent sandbox bypass via traversal (e.g., `../../data/memory.db`).
    fn resolve_path(&self, path_str: &str) -> PathBuf {
        let p = Path::new(path_str);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace_path.join(p)
        };
        normalize_path(&joined)
    }

    // --- Built-in tool implementations ---

    async fn exec_bash(&self, args: &Value) -> ToolResult {
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if command.is_empty() {
            return ToolResult {
                content: "Error: 'command' parameter is required".to_string(),
                is_error: true,
            };
        }

        debug!("tool/bash: {command}");

        let mut cmd =
            match kernex_sandbox::try_protected_command("bash", &self.scope, &self.sandbox_profile)
            {
                Ok(c) => c,
                Err(e) => {
                    return ToolResult {
                        content: format!("Refusing to run bash: {e}"),
                        is_error: true,
                    }
                }
            };
        cmd.arg("-c").arg(command);
        cmd.current_dir(&self.workspace_path);
        // Kill the child process when the handle is dropped (e.g. on timeout).
        cmd.kill_on_drop(true);

        // Capture output with timeout. kill_on_drop ensures no orphan processes.
        match tokio::time::timeout(
            std::time::Duration::from_secs(BASH_TIMEOUT_SECS),
            cmd.output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = format!("(exit code: {})", output.status.code().unwrap_or(-1));
                }
                let is_error = !output.status.success();
                ToolResult {
                    content: truncate_output(&result, MAX_BASH_OUTPUT),
                    is_error,
                }
            }
            Ok(Err(e)) => ToolResult {
                content: format!("Failed to execute command: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: format!("Command timed out after {BASH_TIMEOUT_SECS}s"),
                is_error: true,
            },
        }
    }

    async fn exec_toolbox(&self, tb: &Toolbox, args: &Value) -> ToolResult {
        debug!("toolbox/{}: running", tb.name);

        // Execution-time enforcement of the owning skill's declared command
        // allow-list. The loaders already reject undeclared commands, but
        // this is the last gate before the spawn: a toolbox that reaches
        // the executor through any other path (hand-registered, future
        // loader gap) is still refused here, structurally.
        if !kernex_core::context::command_matches_allowlist(&tb.allowed_commands, &tb.command) {
            return ToolResult {
                content: format!(
                    "Toolbox '{}' blocked: command {:?} is not in the skill's declared command allow-list",
                    tb.name, tb.command
                ),
                is_error: true,
            };
        }

        // Per-tool network opt-in: a toolbox that declares `network = true`
        // gets egress; everything else inherits the executor's
        // deny-by-default posture.
        let mut profile = self.sandbox_profile.clone();
        profile.allow_network = profile.allow_network || tb.network;

        let mut cmd =
            match kernex_sandbox::try_protected_command(&tb.command, &self.scope, &profile) {
                Ok(c) => c,
                Err(e) => {
                    return ToolResult {
                        content: format!("Refusing to run toolbox '{}': {e}", tb.name),
                        is_error: true,
                    }
                }
            };
        cmd.args(&tb.args);
        cmd.current_dir(&self.workspace_path);
        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Declared env passthrough: the spawn boundary cleared the inherited
        // environment; re-add ONLY the parent vars the skill's permissions
        // named. Applied before the literal env map so explicit configured
        // values still win on collision.
        for key in &tb.env_passthrough {
            if kernex_core::spawn::is_unsafe_env_key(key) {
                tracing::warn!(
                    toolbox = %tb.name,
                    env_key = %key,
                    "toolbox: refusing dynamic-linker env passthrough (would bypass sandbox)"
                );
                continue;
            }
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        let (safe_env, dropped) = kernex_core::spawn::filter_unsafe_env(&tb.env);
        for k in &dropped {
            tracing::warn!(
                toolbox = %tb.name,
                env_key = %k,
                "toolbox: dropping dynamic-linker env var supplied by skill (would bypass sandbox)"
            );
        }
        for (k, v) in &safe_env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to spawn toolbox '{}': {e}", tb.name),
                    is_error: true,
                };
            }
        };

        // Write arguments as JSON to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            // Don't silently substitute "" if serialization fails — that
            // would feed the script empty input and produce a confusing
            // downstream error. Surface the failure as a tool error.
            match serde_json::to_string(args) {
                Ok(json) => {
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, json.as_bytes()).await;
                }
                Err(e) => {
                    let _ = child.kill().await;
                    return ToolResult {
                        content: format!("Failed to serialize args for toolbox '{}': {e}", tb.name),
                        is_error: true,
                    };
                }
            }
            // Drop stdin to signal EOF.
        }

        match tokio::time::timeout(
            std::time::Duration::from_secs(TOOLBOX_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = format!("(exit code: {})", output.status.code().unwrap_or(-1));
                }
                ToolResult {
                    content: truncate_output(&result, MAX_BASH_OUTPUT),
                    is_error: !output.status.success(),
                }
            }
            Ok(Err(e)) => ToolResult {
                content: format!("Toolbox '{}' execution failed: {e}", tb.name),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: format!(
                    "Toolbox '{}' timed out after {TOOLBOX_TIMEOUT_SECS}s",
                    tb.name
                ),
                is_error: true,
            },
        }
    }
}

/// Check whether a command exists in PATH without spawning a shell.
fn which_exists(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(cmd).is_file()))
        .unwrap_or(false)
}

/// Lexically normalize a path by resolving `.` and `..` components.
///
/// Unlike `fs::canonicalize`, this works on non-existent paths. Essential
/// for preventing sandbox bypass via `../../` traversal on paths that
/// don't exist on disk.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

/// Truncate output to at most `max_bytes` bytes at a valid UTF-8 char boundary,
/// appending a note if truncated.
pub fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        let boundary = kernex_core::utf8::floor_char_boundary(s, max_bytes);
        let truncated = &s[..boundary];
        format!(
            "{truncated}\n\n... (output truncated: {} total bytes, showing first {boundary})",
            s.len()
        )
    }
}

/// Cached builtin tool definitions — `schemars` reflection runs once per process.
static BUILTIN_TOOL_DEFS_CACHE: std::sync::OnceLock<Vec<ToolDef>> = std::sync::OnceLock::new();

/// Return the definitions of the 7 built-in tools.
///
/// Schema generation runs once per process via `OnceLock`; subsequent calls
/// clone from the cache. This avoids repeated `schemars` reflection and
/// `serde_json` serialization on every provider request.
pub fn builtin_tool_defs() -> Vec<ToolDef> {
    use crate::tool_params::{
        tool_schema_for, BashParams, EditParams, GlobParams, GrepParams, ReadParams,
        WebFetchParams, WriteParams,
    };

    BUILTIN_TOOL_DEFS_CACHE
        .get_or_init(|| {
            vec![
                ToolDef {
                    name: "bash".to_string(),
                    description: "Execute a bash command and return its output.".to_string(),
                    parameters: tool_schema_for::<BashParams>(),
                },
                ToolDef {
                    name: "read".to_string(),
                    description: "Read the contents of a file.".to_string(),
                    parameters: tool_schema_for::<ReadParams>(),
                },
                ToolDef {
                    name: "write".to_string(),
                    description: "Write content to a file (creates or overwrites).".to_string(),
                    parameters: tool_schema_for::<WriteParams>(),
                },
                ToolDef {
                    name: "edit".to_string(),
                    description: "Edit a file by replacing the first occurrence of old_string \
                        with new_string."
                        .to_string(),
                    parameters: tool_schema_for::<EditParams>(),
                },
                ToolDef {
                    name: "grep".to_string(),
                    description:
                        "Search file contents with a regex pattern. Returns matching lines \
                        with file paths and line numbers."
                            .to_string(),
                    parameters: tool_schema_for::<GrepParams>(),
                },
                ToolDef {
                    name: "glob".to_string(),
                    description: "Find files matching a glob pattern (e.g. \"**/*.rs\"). \
                        Returns matching file paths sorted by modification time."
                        .to_string(),
                    parameters: tool_schema_for::<GlobParams>(),
                },
                ToolDef {
                    name: "web_fetch".to_string(),
                    description: "Fetch a URL and return its text content.".to_string(),
                    parameters: tool_schema_for::<WebFetchParams>(),
                },
            ]
        })
        .clone()
}

// --- Shared provider utilities ---

/// Per-response token-usage breakdown. Populated by providers that report
/// the dimensions individually (Anthropic prompt cache, OpenAI/DeepSeek
/// `prompt_tokens_details.cached_tokens`, etc.). All fields are optional so
/// providers without cache support can omit them.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageBreakdown {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

/// Build the standard Response for agentic loop responses.
///
/// Used by all HTTP provider agentic loops (success path and max-turns path).
pub(crate) fn build_response(
    text: String,
    provider_name: &str,
    total_tokens: u64,
    elapsed_ms: u64,
    model: Option<String>,
) -> Response {
    build_response_with_usage(
        text,
        provider_name,
        total_tokens,
        elapsed_ms,
        model,
        UsageBreakdown::default(),
    )
}

/// Like [`build_response`], but also records a per-dimension token breakdown
/// (input / output / cache_read / cache_creation). Providers that expose
/// prompt-cache stats use this variant; the rest stay on the simpler form.
pub(crate) fn build_response_with_usage(
    text: String,
    provider_name: &str,
    total_tokens: u64,
    elapsed_ms: u64,
    model: Option<String>,
    usage: UsageBreakdown,
) -> Response {
    Response {
        text,
        metadata: CompletionMeta {
            provider_used: provider_name.to_string(),
            tokens_used: if total_tokens > 0 {
                Some(total_tokens)
            } else {
                None
            },
            processing_time_ms: elapsed_ms,
            model,
            stop_reason: None,
            session_id: None,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
        },
    }
}

/// Check whether tools are enabled for this request context.
pub(crate) fn tools_enabled(context: &kernex_core::context::Context) -> bool {
    context
        .allowed_tools
        .as_ref()
        .map(|t| !t.is_empty())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_fetch_url_accepts_public_https() {
        assert!(is_safe_fetch_url("https://example.com").is_ok());
        assert!(is_safe_fetch_url("https://api.github.com/repos/foo/bar").is_ok());
        assert!(is_safe_fetch_url("https://1.1.1.1/dns-query").is_ok());
    }

    #[test]
    fn safe_fetch_url_rejects_non_https() {
        assert!(is_safe_fetch_url("http://example.com").is_err());
        assert!(is_safe_fetch_url("ftp://example.com").is_err());
        assert!(is_safe_fetch_url("file:///etc/passwd").is_err());
        assert!(is_safe_fetch_url("not-a-url").is_err());
    }

    #[test]
    fn safe_fetch_url_rejects_metadata_and_loopback() {
        assert!(is_safe_fetch_url("https://169.254.169.254/latest/meta-data").is_err());
        assert!(is_safe_fetch_url("https://127.0.0.1/").is_err());
        assert!(is_safe_fetch_url("https://[::1]/").is_err());
        assert!(is_safe_fetch_url("https://10.0.0.5/").is_err());
        assert!(is_safe_fetch_url("https://192.168.1.1/").is_err());
        assert!(is_safe_fetch_url("https://172.16.0.5/").is_err());
        assert!(is_safe_fetch_url("https://[fe80::1]/").is_err());
        assert!(is_safe_fetch_url("https://[fd00::1]/").is_err());
    }

    #[test]
    fn test_builtin_tool_defs_cached() {
        // Multiple calls must return identical results (cache hit path).
        let first = builtin_tool_defs();
        let second = builtin_tool_defs();
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.parameters, b.parameters);
        }
    }

    #[test]
    fn test_builtin_tool_defs_count() {
        let defs = builtin_tool_defs();
        assert_eq!(defs.len(), 7);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"write"));
        assert!(names.contains(&"edit"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"web_fetch"));
    }

    #[test]
    fn test_tool_def_serialization() {
        let defs = builtin_tool_defs();
        for def in &defs {
            let json = serde_json::to_value(def).unwrap();
            assert!(json.get("name").is_some());
            assert!(json.get("description").is_some());
            assert!(json.get("parameters").is_some());
        }
    }

    #[test]
    fn test_truncate_output_short() {
        let s = "hello world";
        assert_eq!(truncate_output(s, 100), "hello world");
    }

    #[test]
    fn test_truncate_output_exact() {
        let s = "abcde";
        assert_eq!(truncate_output(s, 5), "abcde");
    }

    #[test]
    fn test_truncate_output_long() {
        let s = "a".repeat(100);
        let result = truncate_output(&s, 50);
        assert!(result.starts_with(&"a".repeat(50)));
        assert!(result.contains("output truncated"));
        assert!(result.contains("100 total bytes"));
    }

    #[tokio::test]
    async fn test_exec_bash_empty_command() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let result = executor.exec_bash(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_exec_bash_echo() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let result = executor
            .exec_bash(&serde_json::json!({"command": "echo hello"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_web_fetch_rejects_hostname_resolving_private() {
        // SSRF resolve-time check: "localhost" passes the syntactic guard
        // (it is a hostname, not a literal IP) but resolves to loopback; the
        // resolver-level validation must reject it before any request is
        // sent. Uses the system resolver only - no network egress.
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"))
            .with_web_fetch_policy(WebFetchPolicy::AllowAll);
        let result = executor
            .execute(
                "web_fetch",
                &serde_json::json!({"url": "https://localhost/"}),
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("non-public address")
                || result.content.contains("could not resolve"),
            "expected resolver-level rejection, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_bash_subprocess_env_is_isolated() {
        // Reality gate: a real tool subprocess must not inherit provider keys
        // from the parent, while still receiving the base allowlist.
        std::env::set_var("ANTHROPIC_API_KEY", "kernex-test-sentinel-not-a-real-key");
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let result = executor
            .exec_bash(&serde_json::json!({"command": "env"}))
            .await;
        std::env::remove_var("ANTHROPIC_API_KEY");

        assert!(!result.is_error, "env probe failed: {}", result.content);
        assert!(
            !result
                .content
                .contains("kernex-test-sentinel-not-a-real-key"),
            "provider key leaked into the subprocess environment"
        );
        assert!(result.content.contains("PATH="), "base allowlist lost PATH");
        assert!(result.content.contains("HOME="), "base allowlist lost HOME");
    }

    #[tokio::test]
    async fn test_bash_honors_required_enforcement() {
        // Reality gate for the fail-mode policy: with enforcement required,
        // a real bash subprocess either runs sandboxed (host can enforce -
        // the CI case on macOS Seatbelt and Linux Landlock) or is refused
        // outright (host cannot enforce). It must never silently fall back
        // to an unsandboxed run.
        let executor = ToolExecutor::new(PathBuf::from("/tmp")).with_sandbox_profile(
            kernex_sandbox::SandboxProfile::new().with_require_os_enforcement(true),
        );
        let result = executor
            .exec_bash(&serde_json::json!({"command": "echo enforced-run"}))
            .await;

        if kernex_sandbox::os_enforcement_available() {
            assert!(
                !result.is_error,
                "sandboxed run failed on an enforcement-capable host: {}",
                result.content
            );
            assert!(result.content.contains("enforced-run"));
        } else {
            assert!(
                result.is_error,
                "unsandboxed fallback happened despite required enforcement"
            );
            assert!(
                result.content.contains("Refusing to run bash"),
                "unexpected error shape: {}",
                result.content
            );
        }
    }

    #[tokio::test]
    async fn test_exec_read_nonexistent() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/tmp/kernex_test_nonexistent_xyz"}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_exec_write_and_read() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let path = "/tmp/kernex_tool_test_write.txt";
        let write_result = executor
            .exec_write(&serde_json::json!({"file_path": path, "content": "test content"}))
            .await;
        assert!(!write_result.is_error);

        let read_result = executor
            .exec_read(&serde_json::json!({"file_path": path}))
            .await;
        assert!(!read_result.is_error);
        assert_eq!(read_result.content, "test content");

        // Cleanup.
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_exec_edit() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let path = "/tmp/kernex_tool_test_edit.txt";
        tokio::fs::write(path, "hello world").await.unwrap();

        let result = executor
            .exec_edit(&serde_json::json!({
                "file_path": path,
                "old_string": "world",
                "new_string": "kernex"
            }))
            .await;
        assert!(!result.is_error);

        let content = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(content, "hello kernex");

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_exec_read_denied_protected_path() {
        let executor = kernex_home_executor();
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/home/user/.kernex/data/memory.db"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_read_denied_config() {
        let executor = kernex_home_executor();
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/home/user/.kernex/config.toml"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_write_denied_protected_path() {
        let executor = kernex_home_executor();
        let result = executor
            .exec_write(&serde_json::json!({"file_path": "/home/user/.kernex/data/memory.db", "content": "x"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[test]
    fn test_tool_executor_mcp_tool_map_routing() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        assert!(executor.mcp_tool_map.is_empty());
        assert!(executor.mcp_clients.is_empty());
    }

    #[tokio::test]
    async fn test_allowlist_limits_what_is_offered_and_run() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        std::fs::write(ws.join("a.txt"), "content").unwrap();

        let mut open = ToolExecutor::new(ws.clone());
        assert_eq!(open.all_tool_defs().len(), 6);
        assert!(
            !open
                .execute("read", &serde_json::json!({"file_path": "a.txt"}))
                .await
                .is_error
        );

        let mut limited =
            ToolExecutor::new(ws.clone()).with_allowed_tools(Some(vec!["Read".to_string()]));
        let names: Vec<String> = limited
            .all_tool_defs()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["read".to_string()]);
        assert!(
            !limited
                .execute("read", &serde_json::json!({"file_path": "a.txt"}))
                .await
                .is_error
        );
        let refused = limited
            .execute(
                "write",
                &serde_json::json!({"file_path": "b.txt", "content": "x"}),
            )
            .await;
        assert!(refused.is_error && refused.content.contains("not allowed"));
        assert!(!ws.join("b.txt").exists());

        // Spellings a host may carry over from an agent CLI's configuration.
        let styled = ToolExecutor::new(ws.clone()).with_allowed_tools(Some(vec![
            "Bash(git *)".to_string(),
            " Read".to_string(),
            "WebFetch".to_string(),
            "mcp__srv__deploy".to_string(),
        ]));
        let names: Vec<String> = styled.all_tool_defs().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["bash".to_string(), "read".to_string()]);
        assert!(styled.tool_allowed("web_fetch"));
        assert!(!styled.tool_allowed("deploy"), "no MCP server owns it");
        assert!(!styled.tool_allowed("write"));

        let mut none = ToolExecutor::new(ws).with_allowed_tools(Some(Vec::new()));
        assert!(none.all_tool_defs().is_empty());
        assert!(
            none.execute("read", &serde_json::json!({"file_path": "a.txt"}))
                .await
                .is_error
        );
    }

    #[tokio::test]
    async fn test_permission_rule_sees_the_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(ws.join("private")).unwrap();
        std::fs::write(ws.join("private/key"), "k").unwrap();
        std::fs::write(ws.join("public.txt"), "p").unwrap();
        let rules = kernex_core::permissions::PermissionRules {
            allow: vec![],
            deny: vec![format!("Read({}/private/*)", ws.display())],
        };
        let mut tools = ToolExecutor::new(ws.clone())
            .with_permission_rules_opt(Some(std::sync::Arc::new(rules)));
        // The rule names the absolute directory; relative spellings of a
        // path inside it are denied all the same.
        for path in ["private/key", "./private/key", "sub/../private/key"] {
            let result = tools
                .execute("read", &serde_json::json!({"file_path": path}))
                .await;
            assert!(
                result.is_error && result.content.contains("denied by rule"),
                "{path}"
            );
        }
        // A symlink to the denied directory, and the workspace reached under
        // another name, name the same file.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ws.join("private"), ws.join("link")).unwrap();
            let alias = dir.path().join("alias");
            std::os::unix::fs::symlink(&ws, &alias).unwrap();
            for path in [
                "link/key".to_string(),
                format!("{}/private/key", alias.display()),
            ] {
                let result = tools
                    .execute("read", &serde_json::json!({"file_path": path}))
                    .await;
                assert!(
                    result.is_error && result.content.contains("denied by rule"),
                    "{path}"
                );
            }
        }
        // A search that starts at the directory, above it, or with no path
        // does not return what is inside.
        std::fs::write(ws.join("private/note.txt"), "needle-private").unwrap();
        std::fs::write(ws.join("public.txt"), "needle-public").unwrap();
        for args in [
            serde_json::json!({"pattern": "needle"}),
            serde_json::json!({"pattern": "needle", "path": "."}),
            serde_json::json!({"pattern": "needle", "path": "private"}),
        ] {
            let found = tools.execute("grep", &args).await;
            assert!(
                !found.content.contains("needle-private"),
                "{args}: {}",
                found.content
            );
        }
        let found = tools
            .execute("grep", &serde_json::json!({"pattern": "needle"}))
            .await;
        assert!(found.content.contains("needle-public"), "{}", found.content);
        let listed = tools
            .execute("glob", &serde_json::json!({"pattern": "*.txt"}))
            .await;
        assert!(listed.content.contains("public.txt"), "{}", listed.content);
        assert!(!listed.content.contains("note.txt"), "{}", listed.content);
        let public = tools
            .execute("read", &serde_json::json!({"file_path": "public.txt"}))
            .await;
        assert!(!public.is_error, "{}", public.content);
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let result = executor
            .execute("nonexistent_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown tool"));
    }

    #[test]
    fn test_register_toolboxes() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let toolboxes = vec![Toolbox {
            name: "lint".into(),
            description: "Run linter.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "bash".into(),
            args: vec!["lint.sh".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        }];
        executor.register_toolboxes(&toolboxes);
        assert!(executor.toolboxes.contains_key("lint"));
    }

    #[test]
    fn test_all_tool_defs_includes_toolboxes() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let toolboxes = vec![Toolbox {
            name: "lint".into(),
            description: "Run linter.".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"file": {"type": "string"}}}),
            command: "bash".into(),
            args: vec!["lint.sh".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        }];
        executor.register_toolboxes(&toolboxes);
        let defs = executor.all_tool_defs();
        assert!(defs.iter().any(|d| d.name == "lint"));
        // 6 built-in (web_fetch is off by default) + 1 toolbox
        assert_eq!(defs.len(), 7);
    }

    #[tokio::test]
    async fn test_exec_toolbox_echo() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "greet".into(),
            description: "Echo greeting.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "echo".into(),
            args: vec!["hello from toolbox".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("greet", &serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("hello from toolbox"));
    }

    #[tokio::test]
    async fn test_toolbox_undeclared_command_blocked_at_execution() {
        // Reality gate: a toolbox whose command is NOT in the owning
        // skill's declared allow-list is refused at the execution layer,
        // structurally (no spawn, error result) - not prompt-advisory.
        // This covers any path that bypasses the loaders.
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "sneaky".into(),
            description: "Declared echo, ships sh.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "sh".into(),
            args: vec!["-c".into(), "echo should-never-run".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: vec!["echo".into()],
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("sneaky", &serde_json::json!({})).await;
        assert!(result.is_error, "undeclared command was executed");
        assert!(
            result
                .content
                .contains("not in the skill's declared command allow-list"),
            "unexpected error shape: {}",
            result.content
        );
        assert!(
            !result.content.contains("should-never-run"),
            "blocked toolbox still produced output"
        );
    }

    #[tokio::test]
    async fn test_toolbox_declared_command_runs() {
        // Companion positive case: the same toolbox with its command
        // declared in the allow-list executes normally.
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "honest".into(),
            description: "Declared sh, ships sh.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "sh".into(),
            args: vec!["-c".into(), "echo declared-run".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: vec!["sh".into()],
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("honest", &serde_json::json!({})).await;
        assert!(
            !result.is_error,
            "declared command refused: {}",
            result.content
        );
        assert!(result.content.contains("declared-run"));
    }

    #[tokio::test]
    async fn test_toolbox_env_passthrough_declared_only() {
        // Reality gate for the env permission: a real subprocess receives
        // ONLY the parent vars the skill declared; everything else stays
        // cleared by the spawn boundary, and dynamic-linker names are
        // refused even when declared.
        std::env::set_var("KERNEX_TEST_DECLARED_VAR", "granted-value");
        std::env::set_var("KERNEX_TEST_UNDECLARED_VAR", "must-not-leak");
        std::env::set_var("DYLD_INSERT_LIBRARIES", "/tmp/evil.dylib");

        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "envprobe".into(),
            description: "Print environment.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "/usr/bin/env".into(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: vec![
                "KERNEX_TEST_DECLARED_VAR".into(),
                "DYLD_INSERT_LIBRARIES".into(),
            ],
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("envprobe", &serde_json::json!({})).await;

        std::env::remove_var("KERNEX_TEST_DECLARED_VAR");
        std::env::remove_var("KERNEX_TEST_UNDECLARED_VAR");
        std::env::remove_var("DYLD_INSERT_LIBRARIES");

        assert!(!result.is_error, "env probe failed: {}", result.content);
        assert!(
            result
                .content
                .contains("KERNEX_TEST_DECLARED_VAR=granted-value"),
            "declared passthrough var missing: {}",
            result.content
        );
        assert!(
            !result.content.contains("must-not-leak"),
            "undeclared parent var leaked into the subprocess"
        );
        assert!(
            !result.content.contains("evil.dylib"),
            "dynamic-linker var passed through despite refusal rule"
        );
    }

    #[tokio::test]
    async fn test_exec_toolbox_receives_stdin_json() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "cat-input".into(),
            description: "Cat stdin.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "cat".into(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor
            .execute("cat-input", &serde_json::json!({"file": "test.rs"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("test.rs"));
    }

    #[tokio::test]
    async fn test_exec_toolbox_nonzero_exit() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "fail".into(),
            description: "Always fails.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "bash".into(),
            args: vec!["-c".into(), "echo error >&2; exit 1".into()],
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("fail", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("error"));
    }

    #[tokio::test]
    async fn test_exec_toolbox_with_env() {
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let mut env = std::collections::HashMap::new();
        env.insert("GREETING".into(), "hola".into());
        let tb = Toolbox {
            name: "env-test".into(),
            description: "Print env var.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "bash".into(),
            args: vec!["-c".into(), "echo $GREETING".into()],
            env,
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("env-test", &serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("hola"));
    }

    #[tokio::test]
    async fn test_exec_toolbox_spawn_failure() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"));
        let tb = Toolbox {
            name: "bad".into(),
            description: "Bad command.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "__nonexistent_cmd_xyz__".into(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        let result = executor.exec_toolbox(&tb, &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[test]
    fn test_resolve_path_absolute() {
        let executor = kernex_home_executor();
        let resolved = executor.resolve_path("/tmp/test.txt");
        assert_eq!(resolved, PathBuf::from("/tmp/test.txt"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let executor = kernex_home_executor();
        let resolved = executor.resolve_path("test.txt");
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.kernex/workspace/test.txt")
        );
    }

    #[test]
    fn test_resolve_path_traversal_normalized() {
        let executor = kernex_home_executor();
        let resolved = executor.resolve_path("../data/memory.db");
        assert_eq!(resolved, PathBuf::from("/home/user/.kernex/data/memory.db"));

        let resolved2 = executor.resolve_path("../../data/memory.db");
        assert_eq!(resolved2, PathBuf::from("/home/user/data/memory.db"));
    }

    #[tokio::test]
    async fn test_exec_read_denied_relative_traversal() {
        let executor = kernex_home_executor();
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "../data/memory.db"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_write_denied_config_toml() {
        let executor = kernex_home_executor();
        let result = executor
            .exec_write(
                &serde_json::json!({"file_path": "/home/user/.kernex/config.toml", "content": "x"}),
            )
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    /// An executor whose workspace sits inside its data dir, the layout the
    /// path-denial tests below describe.
    fn kernex_home_executor() -> ToolExecutor {
        let dirs = crate::SandboxDirs::new().with_data_dir("/home/user/.kernex");
        ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace")).with_sandbox_dirs(&dirs)
    }

    #[tokio::test]
    async fn test_exec_write_denied_outside_workspace() {
        let executor = kernex_home_executor();
        for file_path in [
            "/home/user/.kernex/other/file.txt",
            "/home/user/.kernex/workspace-old/file.txt",
            "../sibling/file.txt",
            "/home/user/.zshrc",
        ] {
            let result = executor
                .exec_write(&serde_json::json!({"file_path": file_path, "content": "x"}))
                .await;
            assert!(result.is_error, "{file_path}");
            assert!(result.content.contains("denied"), "{file_path}");
        }
    }

    #[tokio::test]
    async fn test_exec_write_and_edit_denied_in_skills_dir() {
        let executor = kernex_home_executor();
        let skill = "/home/user/.kernex/skills/review/SKILL.md";
        let write = executor
            .exec_write(&serde_json::json!({"file_path": skill, "content": "x"}))
            .await;
        assert!(write.is_error);
        assert!(write.content.contains("denied"));

        let edit = executor
            .exec_edit(&serde_json::json!({
                "file_path": skill,
                "old_string": "a",
                "new_string": "b"
            }))
            .await;
        assert!(edit.is_error);
        assert!(edit.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_read_denied_custom_database() {
        let dirs = crate::SandboxDirs::new()
            .with_data_dir("/home/user/.kernex")
            .with_db_path("/var/lib/agent/memory.db");
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"))
            .with_sandbox_dirs(&dirs);
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/var/lib/agent/memory.db"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[test]
    fn test_with_sandbox_dirs_sets_the_scope() {
        let dirs = crate::SandboxDirs::new()
            .with_data_dir("/opt/kernex/state")
            .with_config_path("/opt/kernex/config.toml");
        let executor = ToolExecutor::new(PathBuf::from("/tmp")).with_sandbox_dirs(&dirs);
        assert_eq!(executor.scope.workspace, PathBuf::from("/tmp"));
        assert_eq!(executor.scope.data_dir, PathBuf::from("/opt/kernex/state"));
        assert_eq!(
            executor.scope.config_path,
            Some(PathBuf::from("/opt/kernex/config.toml"))
        );
    }

    #[test]
    fn test_truncate_output_multibyte_boundary() {
        let s = "\u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{043c}\u{0438}\u{0440}!";
        let result = truncate_output(s, 5);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_truncate_output_emoji_boundary() {
        let s = "Hello \u{1f30d} World";
        let result = truncate_output(s, 8);
        assert!(!result.is_empty());
    }
}
