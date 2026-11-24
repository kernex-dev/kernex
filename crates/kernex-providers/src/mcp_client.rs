//! Minimal MCP (Model Context Protocol) client over stdio transport.
//!
//! Implements JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.
//! No external MCP crate — just raw protocol using tokio + serde.

use futures_util::StreamExt;
use kernex_core::context::McpServer;
use kernex_sandbox::{SandboxProfile, SandboxScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

/// Maximum length of a single JSON-RPC line accepted from an MCP server.
/// MCP servers run in user space; a buggy or hostile one returning a single
/// gigantic line should not be able to OOM the host process.
const MCP_MAX_LINE_LEN: usize = 8 * 1024 * 1024;

/// MCP `protocolVersion` we negotiate during `initialize`. Tracks the
/// published spec at <https://modelcontextprotocol.io/specification>; bump
/// when we adopt a newer spec date and verify our handshake still works
/// against the reference servers. Avoid future-dated values: spec-compliant
/// servers may reject a version they do not recognise.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
use tracing::{debug, warn};

/// Maximum time to wait for a single MCP request/response round-trip.
const MCP_REQUEST_TIMEOUT_SECS: u64 = 120;

/// Errors emitted by the MCP client.
///
/// Defining a typed error here (instead of leaking `anyhow::Error` in public
/// signatures) keeps the option open to promote `mcp_client` from
/// `pub(crate)` to `pub` without churn for downstream consumers.
#[derive(Debug, Error)]
pub enum McpError {
    /// Spawning the MCP server process failed.
    #[error("mcp: failed to spawn '{command}': {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },
    /// The server's command is not in its declared command allow-list, or
    /// no allow-list was declared.
    #[error(
        "mcp: '{server}' not started: command {command:?} is not in a declared command allow-list"
    )]
    NotAllowed { server: String, command: String },
    /// stdin/stdout pipe was missing on the spawned child.
    #[error("mcp: '{server}' missing {channel} pipe")]
    MissingPipe {
        server: String,
        channel: &'static str,
    },
    /// I/O error reading from or writing to the server.
    #[error("mcp: I/O error on '{server}': {source}")]
    Io {
        server: String,
        #[source]
        source: std::io::Error,
    },
    /// Failed to (de)serialize a JSON-RPC frame.
    #[error("mcp: JSON error on '{server}': {source}")]
    Json {
        server: String,
        #[source]
        source: serde_json::Error,
    },
    /// MCP server returned a JSON-RPC error response.
    #[error("mcp: '{server}' error on {method}: {message}")]
    Server {
        server: String,
        method: String,
        message: String,
    },
    /// MCP server's stdout closed mid-request.
    #[error("mcp: '{server}' stdout closed")]
    StdoutClosed { server: String },
    /// Round-trip exceeded [`MCP_REQUEST_TIMEOUT_SECS`].
    #[error("mcp: '{server}' timed out waiting for response to {method} (>{seconds}s)")]
    Timeout {
        server: String,
        method: String,
        seconds: u64,
    },
}

/// A tool definition discovered from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Tool name (e.g. "browser_navigate").
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the tool's parameters.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// Result of calling a tool.
#[derive(Debug, Clone)]
pub struct McpToolResult {
    /// Text content from the tool response.
    pub content: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// MCP client connected to a single server process via stdio.
pub struct McpClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: FramedRead<ChildStdout, LinesCodec>,
    next_id: u64,
    server_name: String,
    /// Tools discovered via `tools/list`.
    pub tools: Vec<McpToolDef>,
}

// --- JSON-RPC types (private) ---

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

impl McpClient {
    /// Connect to an MCP server by spawning its process inside the sandbox.
    ///
    /// The server runs under the same scope as a tool subprocess: it writes
    /// to the workspace and nowhere in the runtime data directory, and does
    /// not read credential stores. It gets network access only when its
    /// declaration (or the host's profile) grants it. A server whose command
    /// is not in a non-empty declared allow-list is not started.
    ///
    /// Performs the full initialization handshake:
    /// 1. Spawn process with piped stdin/stdout
    /// 2. Send `initialize` request
    /// 3. Send `notifications/initialized` notification
    /// 4. Send `tools/list` to discover available tools
    pub async fn connect(
        server: &McpServer,
        scope: &SandboxScope,
        profile: &SandboxProfile,
    ) -> Result<Self, McpError> {
        let name = server.name.as_str();
        let command = server.command.as_str();
        if !kernex_core::context::mcp_command_allowed(&server.allowed_commands, command) {
            return Err(McpError::NotAllowed {
                server: name.to_string(),
                command: command.to_string(),
            });
        }
        debug!(
            "mcp: connecting to server '{name}' via: {command} {}",
            server.args.join(" ")
        );

        // The sandbox wrappers leave stdio alone (`sandbox-exec` passes it
        // through, the Landlock hook runs before exec), so the JSON-RPC
        // framing is unaffected. The wrapper also clears the inherited
        // environment (provider API keys included); the server's configured
        // env below is the explicit opt-in.
        let profile = server_profile(profile, server);
        let mut cmd =
            kernex_sandbox::try_protected_command(command, scope, &profile).map_err(|e| {
                McpError::Spawn {
                    command: command.to_string(),
                    source: e,
                }
            })?;
        cmd.args(&server.args)
            .current_dir(&scope.workspace)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let (safe_env, dropped) = kernex_core::spawn::filter_unsafe_env(&server.env);
        for k in &dropped {
            tracing::warn!(
                server = name,
                env_key = %k,
                "mcp: dropping dynamic-linker env var supplied by skill (would bypass sandbox)"
            );
        }
        for (k, v) in &safe_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| McpError::Spawn {
            command: command.to_string(),
            source: e,
        })?;

        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| McpError::MissingPipe {
            server: name.to_string(),
            channel: "stdin",
        })?);
        let stdout_pipe = child.stdout.take().ok_or_else(|| McpError::MissingPipe {
            server: name.to_string(),
            channel: "stdout",
        })?;
        let stdout = FramedRead::new(
            stdout_pipe,
            LinesCodec::new_with_max_length(MCP_MAX_LINE_LEN),
        );

        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            server_name: name.to_string(),
            tools: Vec::new(),
        };

        // Step 2: initialize
        let init_params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "kernex",
                "version": "0.1.0"
            }
        });
        let _init_resp = client.request("initialize", Some(init_params)).await?;
        debug!("mcp: '{name}' initialized");

        // Step 3: notifications/initialized (no id = notification)
        client.notify("notifications/initialized", None).await?;

        // Step 4: tools/list
        let tools_resp = client.request("tools/list", None).await?;
        client.tools = parse_tools_list(&tools_resp);
        debug!("mcp: '{name}' discovered {} tools", client.tools.len());

        Ok(client)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<McpToolResult, McpError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        });
        let resp = self.request("tools/call", Some(params)).await?;
        Ok(parse_tool_result(&resp))
    }

    /// Gracefully shut down the MCP server.
    pub async fn shutdown(mut self) {
        // Kill the process. Tokio will close stdin/stdout on drop.
        let _ = self.child.start_kill();

        // Wait briefly for it to exit.
        tokio::select! {
            _ = self.child.wait() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                warn!("mcp: '{}' did not exit after kill", self.server_name);
            }
        }
    }

    /// Send a JSON-RPC request (with id) and read the response.
    ///
    /// Times out after [`MCP_REQUEST_TIMEOUT_SECS`] to prevent infinite hangs
    /// from misbehaving servers.
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req).map_err(|e| McpError::Json {
            server: self.server_name.clone(),
            source: e,
        })?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Io {
                server: self.server_name.clone(),
                source: e,
            })?;
        self.stdin.flush().await.map_err(|e| McpError::Io {
            server: self.server_name.clone(),
            source: e,
        })?;

        // Read lines until we get a response with our id, with a timeout guard.
        let server_name = self.server_name.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS),
            Self::read_response(&mut self.stdout, id, &server_name, method),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(McpError::Timeout {
                server: self.server_name.clone(),
                method: method.to_string(),
                seconds: MCP_REQUEST_TIMEOUT_SECS,
            }),
        }
    }

    /// Read stdout lines until a JSON-RPC response matching `id` arrives.
    async fn read_response(
        stdout: &mut FramedRead<ChildStdout, LinesCodec>,
        id: u64,
        server_name: &str,
        method: &str,
    ) -> Result<Value, McpError> {
        loop {
            let raw = match stdout.next().await {
                Some(Ok(line)) => line,
                Some(Err(LinesCodecError::MaxLineLengthExceeded)) => {
                    return Err(McpError::Server {
                        server: server_name.to_string(),
                        method: method.to_string(),
                        message: format!(
                            "stdout line exceeded {MCP_MAX_LINE_LEN} bytes — refusing to buffer further"
                        ),
                    });
                }
                Some(Err(LinesCodecError::Io(e))) => {
                    return Err(McpError::Io {
                        server: server_name.to_string(),
                        source: e,
                    });
                }
                None => {
                    return Err(McpError::StdoutClosed {
                        server: server_name.to_string(),
                    });
                }
            };

            // Skip empty lines.
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }

            let resp: JsonRpcResponse = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(_) => continue, // Skip non-JSON lines (notifications, etc.).
            };

            // Check if this response matches our request id.
            let resp_id = resp.id.as_ref().and_then(|v| v.as_u64());
            if resp_id != Some(id) {
                continue; // Not our response, skip.
            }

            if let Some(err) = resp.error {
                return Err(McpError::Server {
                    server: server_name.to_string(),
                    method: method.to_string(),
                    message: err.message,
                });
            }

            return Ok(resp.result.unwrap_or(Value::Null));
        }
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: None,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req).map_err(|e| McpError::Json {
            server: self.server_name.clone(),
            source: e,
        })?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Io {
                server: self.server_name.clone(),
                source: e,
            })?;
        self.stdin.flush().await.map_err(|e| McpError::Io {
            server: self.server_name.clone(),
            source: e,
        })?;
        Ok(())
    }
}

/// Parse the `tools/list` response into `McpToolDef` items.
/// The sandbox profile one server runs under: the executor's profile, with
/// network access when the server's declaration grants it.
fn server_profile(base: &SandboxProfile, server: &McpServer) -> SandboxProfile {
    let mut profile = base.clone();
    profile.allow_network = profile.allow_network || server.network;
    profile
}

fn parse_tools_list(result: &Value) -> Vec<McpToolDef> {
    result
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| serde_json::from_value::<McpToolDef>(t.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `tools/call` response into a `McpToolResult`.
fn parse_tool_result(result: &Value) -> McpToolResult {
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let content = result
        .get("content")
        .and_then(|v| v.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    McpToolResult { content, is_error }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stdio MCP server in POSIX shell: answers the handshake and offers
    /// `cat_file` (print a file) and `put_file` (create one). Each replies
    /// `DENIED` when the operating system refuses.
    const ECHO_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"cat_file","description":"print a file","inputSchema":{"type":"object"}},{"name":"put_file","description":"create a file","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"name":"cat_file"'*)
      if text=$(cat "$path" 2>/dev/null); then :; else text=DENIED; fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}]}}\n' "$id" "$text" ;;
    *'"name":"put_file"'*)
      if (printf made > "$path") 2>/dev/null; then text=WROTE; else text=DENIED; fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}]}}\n' "$id" "$text" ;;
  esac
done
"#;

    /// A directory beside the test binary. The system temp directory is
    /// writable by policy and would hide a write denial.
    fn fixture_dir(name: &str) -> std::path::PathBuf {
        let exe = std::env::current_exe().unwrap();
        let dir = exe
            .parent()
            .unwrap()
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn echo_server(script: &std::path::Path, allowed: &[&str]) -> McpServer {
        McpServer {
            name: "echo".into(),
            command: "sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            allowed_commands: allowed.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    async fn call(client: &mut McpClient, tool: &str, path: &std::path::Path) -> String {
        client
            .call_tool(tool, &serde_json::json!({ "path": path }))
            .await
            .unwrap()
            .content
    }

    /// A real MCP server started by the client runs inside the sandbox: it
    /// serves requests, and cannot read a protected file or the memory
    /// database, nor write outside the workspace.
    #[tokio::test]
    async fn test_server_runs_inside_the_sandbox() {
        let root = fixture_dir("mcp-sandbox");
        let workspace = root.join("proj");
        let data_dir = root.join("data");
        let secret = root.join("secrets/token");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(secret.parent().unwrap()).unwrap();
        std::fs::write(workspace.join("server.sh"), ECHO_SERVER).unwrap();
        std::fs::write(workspace.join("note.txt"), "workspace-note").unwrap();
        std::fs::write(&secret, "credential-fixture").unwrap();
        std::fs::write(data_dir.join("memory.db"), "memory-fixture").unwrap();

        let scope = SandboxScope::new(&workspace, &data_dir);
        let profile = SandboxProfile::default()
            .with_blocked_paths(vec![secret.parent().unwrap().to_path_buf()]);
        let script = workspace.join("server.sh");

        // Nothing declared, or another command declared: not started.
        for allowed in [&[][..], &["npx"][..]] {
            let refused =
                McpClient::connect(&echo_server(&script, allowed), &scope, &profile).await;
            assert!(matches!(refused, Err(McpError::NotAllowed { .. })));
        }

        let mut client = McpClient::connect(&echo_server(&script, &["sh"]), &scope, &profile)
            .await
            .unwrap();
        let names: Vec<&str> = client.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["cat_file", "put_file"]);
        assert_eq!(
            call(&mut client, "cat_file", &workspace.join("note.txt")).await,
            "workspace-note"
        );
        assert_eq!(
            call(&mut client, "put_file", &workspace.join("out.txt")).await,
            "WROTE"
        );

        let required =
            std::env::var("KERNEX_TEST_REQUIRE_SANDBOX").is_ok_and(|v| !v.is_empty() && v != "0");
        if !kernex_sandbox::os_enforcement_available() {
            assert!(!required, "no OS-level sandbox enforcement on this host");
            eprintln!("skipping the denial probes: no OS-level sandbox enforcement");
        } else {
            assert_eq!(call(&mut client, "cat_file", &secret).await, "DENIED");
            assert_eq!(
                call(&mut client, "cat_file", &data_dir.join("memory.db")).await,
                "DENIED"
            );
            for outside in [root.join("outside.txt"), data_dir.join("planted.txt")] {
                assert_eq!(call(&mut client, "put_file", &outside).await, "DENIED");
                assert!(!outside.exists(), "{}", outside.display());
            }
        }
        client.shutdown().await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Manual probe of a published server started the way the example
    /// skills start one. Downloads an npm package, so it is not part of the
    /// suite: `cargo test -p kernex-providers real_npx -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "downloads an npm package; run by hand"]
    async fn real_npx_server_starts_inside_the_sandbox() {
        let root = fixture_dir("mcp-npx");
        let workspace = root.join("proj");
        std::fs::create_dir_all(&workspace).unwrap();
        let scope = SandboxScope::new(&workspace, root.join("data"));
        let mut server = McpServer {
            name: "filesystem".into(),
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
                ".".into(),
            ],
            allowed_commands: vec!["npx".into()],
            ..Default::default()
        };
        let offline = McpClient::connect(&server, &scope, &SandboxProfile::default()).await;
        eprintln!(
            "without the network (starts only from the npm cache): {:?}",
            offline
                .as_ref()
                .map(|c| c.tools.len())
                .map_err(|e| e.to_string())
        );
        if let Ok(client) = offline {
            client.shutdown().await;
        }
        server.network = true;
        let client = McpClient::connect(&server, &scope, &SandboxProfile::default())
            .await
            .unwrap();
        eprintln!("with the network: {} tools", client.tools.len());
        assert!(!client.tools.is_empty());
        client.shutdown().await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_server_network_follows_its_declaration() {
        let base = SandboxProfile::default();
        let mut server = McpServer::default();
        assert!(!server_profile(&base, &server).allow_network);
        server.network = true;
        assert!(server_profile(&base, &server).allow_network);
        // A host profile that already allows the network keeps it.
        let open = SandboxProfile::default().with_allow_network(true);
        assert!(server_profile(&open, &McpServer::default()).allow_network);
    }

    #[test]
    fn test_jsonrpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: Some(1),
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "tools/list");
        assert!(json.get("params").is_none());
    }

    #[test]
    fn test_jsonrpc_request_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: Some(5),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "bash", "arguments": {"command": "ls"}})),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["params"]["name"], "bash");
    }

    #[test]
    fn test_jsonrpc_notification_no_id() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("id").is_none());
    }

    #[test]
    fn test_jsonrpc_response_parsing() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"bash","description":"Run a command","inputSchema":{"type":"object"}}]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.error.is_none());
        let tools = resp.result.unwrap();
        assert!(tools.get("tools").unwrap().as_array().unwrap().len() == 1);
    }

    #[test]
    fn test_jsonrpc_error_response_parsing() {
        let raw =
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_tools_list_parsing() {
        let result = serde_json::json!({
            "tools": [
                {
                    "name": "browser_navigate",
                    "description": "Navigate to a URL",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "url": { "type": "string" } },
                        "required": ["url"]
                    }
                },
                {
                    "name": "browser_click",
                    "description": "Click an element",
                    "inputSchema": { "type": "object" }
                }
            ]
        });
        let tools = parse_tools_list(&result);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "browser_navigate");
        assert_eq!(tools[1].name, "browser_click");
        assert!(!tools[0].description.is_empty());
    }

    #[test]
    fn test_tool_result_parsing() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "line one"},
                {"type": "text", "text": "line two"}
            ],
            "isError": false
        });
        let tr = parse_tool_result(&result);
        assert_eq!(tr.content, "line one\nline two");
        assert!(!tr.is_error);
    }

    #[test]
    fn test_tool_result_error() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "command failed"}],
            "isError": true
        });
        let tr = parse_tool_result(&result);
        assert!(tr.is_error);
        assert_eq!(tr.content, "command failed");
    }

    #[test]
    fn test_tool_result_empty() {
        let result = serde_json::json!({});
        let tr = parse_tool_result(&result);
        assert!(!tr.is_error);
        assert_eq!(tr.content, "");
    }
}
