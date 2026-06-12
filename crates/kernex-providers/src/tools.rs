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
    data_dir: PathBuf,
    config_path: Option<PathBuf>,
    sandbox_profile: kernex_sandbox::SandboxProfile,
    mcp_clients: HashMap<String, McpClient>,
    mcp_tool_map: HashMap<String, String>,
    toolboxes: HashMap<String, Toolbox>,
    hook_runner: Option<std::sync::Arc<dyn kernex_core::hooks::HookRunner>>,
    permission_rules: Option<std::sync::Arc<kernex_core::permissions::PermissionRules>>,
}

impl ToolExecutor {
    /// Create a new tool executor.
    ///
    /// `workspace_path` is the working directory.
    /// `data_dir` is derived as the parent of `workspace_path`.
    pub fn new(workspace_path: PathBuf) -> Self {
        // data_dir = parent of workspace
        let data_dir = workspace_path
            .parent()
            .unwrap_or(&workspace_path)
            .to_path_buf();
        Self {
            workspace_path,
            data_dir,
            config_path: None,
            sandbox_profile: Default::default(),
            mcp_clients: HashMap::new(),
            mcp_tool_map: HashMap::new(),
            toolboxes: HashMap::new(),
            hook_runner: None,
            permission_rules: None,
        }
    }

    /// Set the config file path for read protection.
    ///
    /// When set, the sandbox will block AI tool reads to this path,
    /// protecting API keys and secrets even when config lives outside `data_dir`.
    /// Standard `{data_dir}/config.toml` is always protected regardless.
    #[allow(dead_code)]
    pub fn with_config_path(mut self, config_path: PathBuf) -> Self {
        self.config_path = Some(config_path);
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

        defs
    }

    /// Execute a tool call by name, routing to built-in or MCP.
    ///
    /// Fires pre/post hooks when a [`HookRunner`] is attached. A blocked
    /// pre-hook short-circuits execution and returns an error result.
    ///
    /// [`HookRunner`]: kernex_core::hooks::HookRunner
    pub async fn execute(&mut self, tool_name: &str, args: &Value) -> ToolResult {
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
            if let kernex_core::permissions::PermissionOutcome::Deny(reason) =
                rules.check(tool_name, args)
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

        let mut cmd = match kernex_sandbox::try_protected_command(
            "bash",
            &self.data_dir,
            &self.sandbox_profile,
        ) {
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

    async fn exec_read(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if path_str.is_empty() {
            return ToolResult {
                content: "Error: 'file_path' parameter is required".to_string(),
                is_error: true,
            };
        }

        // Resolve relative paths against workspace to prevent sandbox bypass.
        let path = self.resolve_path(path_str);
        let path = path.as_path();
        if kernex_sandbox::is_read_blocked(
            path,
            &self.data_dir,
            self.config_path.as_deref(),
            Some(&self.sandbox_profile),
        ) {
            return ToolResult {
                content: format!("Read denied: {} is a protected path", path.display()),
                is_error: true,
            };
        }

        debug!("tool/read: {}", path.display());

        match tokio::fs::read_to_string(path).await {
            Ok(content) => ToolResult {
                content: truncate_output(&content, MAX_READ_OUTPUT),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Error reading {}: {e}", path.display()),
                is_error: true,
            },
        }
    }

    async fn exec_write(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if path_str.is_empty() {
            return ToolResult {
                content: "Error: 'file_path' parameter is required".to_string(),
                is_error: true,
            };
        }

        // Resolve relative paths against workspace to prevent sandbox bypass.
        let path = self.resolve_path(path_str);
        let path = path.as_path();
        if kernex_sandbox::is_write_blocked(path, &self.data_dir, Some(&self.sandbox_profile)) {
            return ToolResult {
                content: format!("Write denied: {} is a protected path", path.display(),),
                is_error: true,
            };
        }

        debug!("tool/write: {}", path.display());

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolResult {
                    content: format!("Failed to create parent directory: {e}"),
                    is_error: true,
                };
            }
        }

        match tokio::fs::write(path, content).await {
            Ok(()) => ToolResult {
                content: format!("Wrote {} bytes to {}", content.len(), path.display()),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Error writing {}: {e}", path.display()),
                is_error: true,
            },
        }
    }

    async fn exec_toolbox(&self, tb: &Toolbox, args: &Value) -> ToolResult {
        debug!("toolbox/{}: running", tb.name);

        // Per-tool network opt-in: a toolbox that declares `network = true`
        // gets egress; everything else inherits the executor's
        // deny-by-default posture.
        let mut profile = self.sandbox_profile.clone();
        profile.allow_network = profile.allow_network || tb.network;

        let mut cmd =
            match kernex_sandbox::try_protected_command(&tb.command, &self.data_dir, &profile) {
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

    async fn exec_edit(&self, args: &Value) -> ToolResult {
        let path_str = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let old_string = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_string = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if path_str.is_empty() {
            return ToolResult {
                content: "Error: 'file_path' parameter is required".to_string(),
                is_error: true,
            };
        }
        if old_string.is_empty() {
            return ToolResult {
                content: "Error: 'old_string' parameter is required".to_string(),
                is_error: true,
            };
        }

        // Resolve relative paths against workspace to prevent sandbox bypass.
        let path = self.resolve_path(path_str);
        let path = path.as_path();
        if kernex_sandbox::is_write_blocked(path, &self.data_dir, Some(&self.sandbox_profile)) {
            return ToolResult {
                content: format!("Write denied: {} is a protected path", path.display(),),
                is_error: true,
            };
        }

        debug!("tool/edit: {}", path.display());

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading {}: {e}", path.display()),
                    is_error: true,
                }
            }
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            return ToolResult {
                content: "Error: old_string not found in file".to_string(),
                is_error: true,
            };
        }

        let new_content = content.replacen(old_string, new_string, 1);
        match tokio::fs::write(path, &new_content).await {
            Ok(()) => ToolResult {
                content: format!(
                    "Edited {} ({count} occurrence(s) of pattern, replaced first)",
                    path.display()
                ),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Error writing {}: {e}", path.display()),
                is_error: true,
            },
        }
    }

    /// Run ripgrep (or grep) to search file contents.
    ///
    /// Arguments are passed as a pre-split argv array — no shell interpolation —
    /// to prevent command injection.
    async fn exec_grep(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return ToolResult {
                content: "Error: 'pattern' parameter is required".to_string(),
                is_error: true,
            };
        }
        // Length cap on the pattern to limit ReDoS surface. The 30 s
        // timeout is the ultimate safety net, but a tool-using model that
        // sets `pattern` to a pathological backtracking regex (e.g.
        // `(a+)+$`) on a large repo can still burn 30 s of CPU per call.
        // 1024 chars is generous for any realistic search and shrinks the
        // attack window.
        if pattern.len() > MAX_GREP_PATTERN_LEN {
            return ToolResult {
                content: format!("Error: 'pattern' exceeds {MAX_GREP_PATTERN_LEN}-char limit"),
                is_error: true,
            };
        }

        let search_path = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| self.resolve_path(p))
            .unwrap_or_else(|| self.workspace_path.clone());

        let glob_pattern = args.get("glob").and_then(|v| v.as_str());

        debug!(
            "tool/grep: pattern={pattern} path={}",
            search_path.display()
        );

        // Prefer `rg` (ripgrep) when available; fall back to `grep -r`.
        let (cmd_name, mut argv): (&str, Vec<String>) = if which_exists("rg") {
            let mut a = vec![
                "--line-number".to_string(),
                "--color=never".to_string(),
                // Cap the compiled regex DFA size to 16 MiB. Without this,
                // ripgrep allocates up to 500 MiB by default for complex
                // patterns. 16 MiB is plenty for legitimate searches and
                // bounds the worst-case memory footprint.
                "--regex-size-limit".to_string(),
                "16M".to_string(),
            ];
            if let Some(g) = glob_pattern {
                a.push("--glob".to_string());
                a.push(g.to_string());
            }
            a.push(pattern.to_string());
            a.push(search_path.to_string_lossy().to_string());
            ("rg", a)
        } else {
            let mut a = vec![
                "-r".to_string(),
                "-n".to_string(),
                "--include=*".to_string(),
            ];
            if let Some(g) = glob_pattern {
                // grep --include only supports simple globs; best-effort.
                a.push(format!("--include={g}"));
            }
            a.push(pattern.to_string());
            a.push(search_path.to_string_lossy().to_string());
            ("grep", a)
        };

        let mut cmd = match kernex_sandbox::try_protected_command(
            cmd_name,
            &self.data_dir,
            &self.sandbox_profile,
        ) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Refusing to run {cmd_name}: {e}"),
                    is_error: true,
                }
            }
        };
        cmd.args(&argv);
        cmd.kill_on_drop(true);
        // Drain argv so the borrow checker is happy.
        argv.clear();

        match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // grep/rg exit code 1 = no matches (not an error for us).
                if stdout.is_empty() && !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.is_empty() {
                        ToolResult {
                            content: "(no matches found)".to_string(),
                            is_error: false,
                        }
                    } else {
                        ToolResult {
                            content: format!("grep error: {stderr}"),
                            is_error: true,
                        }
                    }
                } else if stdout.is_empty() {
                    ToolResult {
                        content: "(no matches found)".to_string(),
                        is_error: false,
                    }
                } else {
                    ToolResult {
                        content: truncate_output(&stdout, MAX_READ_OUTPUT),
                        is_error: false,
                    }
                }
            }
            Ok(Err(e)) => ToolResult {
                content: format!("grep execution failed: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: "grep timed out after 30s".to_string(),
                is_error: true,
            },
        }
    }

    /// Walk a directory tree matching files against a glob pattern.
    async fn exec_glob(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return ToolResult {
                content: "Error: 'pattern' parameter is required".to_string(),
                is_error: true,
            };
        }

        let base = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| self.resolve_path(p))
            .unwrap_or_else(|| self.workspace_path.clone());

        debug!("tool/glob: pattern={pattern} base={}", base.display());

        // Shell out to `find` with safe pre-split argv (no shell interpolation).
        // Translate glob to find -name; for recursive patterns (**) use -path.
        let (flag, pat) = if pattern.contains('/') || pattern.contains("**") {
            ("-path", format!("./{pattern}"))
        } else {
            ("-name", pattern.to_string())
        };

        let mut cmd = match kernex_sandbox::try_protected_command(
            "find",
            &self.data_dir,
            &self.sandbox_profile,
        ) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Refusing to run find: {e}"),
                    is_error: true,
                }
            }
        };
        cmd.arg(base.as_os_str())
            .arg(flag)
            .arg(&pat)
            .arg("-not")
            .arg("-path")
            .arg("*/.git/*");
        cmd.kill_on_drop(true);

        match tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.is_empty() {
                    ToolResult {
                        content: "(no files matched)".to_string(),
                        is_error: false,
                    }
                } else {
                    ToolResult {
                        content: truncate_output(&stdout, MAX_READ_OUTPUT),
                        is_error: false,
                    }
                }
            }
            Ok(Err(e)) => ToolResult {
                content: format!("glob execution failed: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: "glob timed out after 15s".to_string(),
                is_error: true,
            },
        }
    }

    /// Fetch a URL and return its text content.
    async fn exec_web_fetch(&self, args: &Value) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            return ToolResult {
                content: "Error: 'url' parameter is required".to_string(),
                is_error: true,
            };
        }

        // SSRF guard: only allow https://, reject loopback / private / link-local
        // / cloud-metadata IP ranges so a tool-using model cannot probe
        // 169.254.169.254, 127.0.0.1:11434 (other agents on the host),
        // 10.x / 192.168.x / fd00::/8 networks. file://, ftp://, gopher://
        // and other schemes are blocked by the https check.
        let parsed = match is_safe_fetch_url(url) {
            Ok(p) => p,
            Err(reason) => {
                return ToolResult {
                    content: format!("Error: refusing to fetch '{url}': {reason}"),
                    is_error: true,
                };
            }
        };

        debug!("tool/web_fetch: {url}");

        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());

        // Resolve-time SSRF pinning: a hostname that passes the syntactic
        // check can still resolve to a private address (attacker-controlled
        // DNS, rebinding). Resolve it here, validate EVERY address, then pin
        // the connection to the validated set so the answer cannot change
        // between check and connect. Literal-IP hosts were already validated
        // above; redirects are disabled, so there are no further hops.
        let host = parsed
            .host_str()
            .unwrap_or("")
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        if host.parse::<std::net::IpAddr>().is_err() {
            let port = parsed.port().unwrap_or(443);
            let addrs: Vec<std::net::SocketAddr> =
                match tokio::net::lookup_host((host.as_str(), port)).await {
                    Ok(it) => it.collect(),
                    Err(e) => {
                        return ToolResult {
                            content: format!("Error: could not resolve '{host}': {e}"),
                            is_error: true,
                        };
                    }
                };
            if addrs.is_empty() {
                return ToolResult {
                    content: format!("Error: '{host}' resolved to no addresses"),
                    is_error: true,
                };
            }
            if let Some(bad) = addrs.iter().find(|a| !is_public_ip(&a.ip())) {
                return ToolResult {
                    content: format!(
                        "Error: refusing to fetch '{url}': host resolves to a non-public address ({})",
                        bad.ip()
                    ),
                    is_error: true,
                };
            }
            builder = builder.resolve_to_addrs(&host, &addrs);
        }

        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to build HTTP client: {e}"),
                    is_error: true,
                }
            }
        };

        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(body) => ToolResult {
                        content: truncate_output(&body, MAX_READ_OUTPUT),
                        is_error: !status.is_success(),
                    },
                    Err(e) => ToolResult {
                        content: format!("Failed to read response body: {e}"),
                        is_error: true,
                    },
                }
            }
            Err(e) => ToolResult {
                content: format!("HTTP request failed: {e}"),
                is_error: true,
            },
        }
    }
}

/// Validate a URL for `web_fetch` against SSRF abuse.
///
/// Returns `Ok(())` only when the URL is `https://`, the host parses to a
/// public IP (or is left as a hostname for which we trust DNS — see notes),
/// and the port is unspecified (443) or in the standard HTTPS range. Cloud
/// metadata IPs (169.254.169.254), loopback, private, link-local, and
/// unspecified addresses are rejected.
///
/// Note: this is best-effort defence. A hostname that resolves at request
/// time to a private IP would still bypass the check; defeating that
/// requires hooking into reqwest's DNS resolver. Catching the obvious
/// literal-IP attempts is the realistic high-value layer.
fn is_safe_fetch_url(url: &str) -> Result<reqwest::Url, &'static str> {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return Err("not a valid URL"),
    };
    if parsed.scheme() != "https" {
        return Err("only https:// is allowed");
    }
    let host = parsed.host_str().ok_or("missing host")?;
    // For IPv6, host_str() returns the address without surrounding brackets in
    // recent reqwest, but older versions wrapped it. Strip just in case.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if !is_public_ip(&ip) {
            return Err("host is a loopback / private / link-local / metadata IP");
        }
    }
    Ok(parsed)
}

fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                // 169.254.169.254 — AWS / GCP / Azure instance metadata.
                || (octets[0] == 169 && octets[1] == 254)
                // 100.64.0.0/10 — carrier-grade NAT, often points at internal services.
                || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64))
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fd00::/8 — unique local addresses.
                || (v6.octets()[0] & 0xfe) == 0xfc
                // fe80::/10 — link-local.
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80))
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
        let mut executor = ToolExecutor::new(PathBuf::from("/tmp"));
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
            kernex_sandbox::SandboxProfile {
                require_os_enforcement: true,
                ..Default::default()
            },
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
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/home/user/.kernex/data/memory.db"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_read_denied_config() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "/home/user/.kernex/config.toml"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_write_denied_protected_path() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
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
            search_hints: Vec::new(),
        }];
        executor.register_toolboxes(&toolboxes);
        let defs = executor.all_tool_defs();
        assert!(defs.iter().any(|d| d.name == "lint"));
        assert_eq!(defs.len(), 8); // 7 built-in + 1 toolbox
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
            search_hints: Vec::new(),
        };
        executor.register_toolboxes(&[tb]);
        let result = executor.execute("greet", &serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("hello from toolbox"));
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
            search_hints: Vec::new(),
        };
        let result = executor.exec_toolbox(&tb, &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[test]
    fn test_resolve_path_absolute() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let resolved = executor.resolve_path("/tmp/test.txt");
        assert_eq!(resolved, PathBuf::from("/tmp/test.txt"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let resolved = executor.resolve_path("test.txt");
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.kernex/workspace/test.txt")
        );
    }

    #[test]
    fn test_resolve_path_traversal_normalized() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let resolved = executor.resolve_path("../data/memory.db");
        assert_eq!(resolved, PathBuf::from("/home/user/.kernex/data/memory.db"));

        let resolved2 = executor.resolve_path("../../data/memory.db");
        assert_eq!(resolved2, PathBuf::from("/home/user/data/memory.db"));
    }

    #[tokio::test]
    async fn test_exec_read_denied_relative_traversal() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let result = executor
            .exec_read(&serde_json::json!({"file_path": "../data/memory.db"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[tokio::test]
    async fn test_exec_write_denied_config_toml() {
        let executor = ToolExecutor::new(PathBuf::from("/home/user/.kernex/workspace"));
        let result = executor
            .exec_write(
                &serde_json::json!({"file_path": "/home/user/.kernex/config.toml", "content": "x"}),
            )
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("denied"));
    }

    #[test]
    fn test_with_config_path() {
        let executor = ToolExecutor::new(PathBuf::from("/tmp"))
            .with_config_path(PathBuf::from("/opt/kernex/config.toml"));
        assert_eq!(
            executor.config_path,
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
