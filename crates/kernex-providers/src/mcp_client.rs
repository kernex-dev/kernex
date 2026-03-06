//! Minimal MCP (Model Context Protocol) client over stdio transport.
//!
//! Implements JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.
//! No external MCP crate — just raw protocol using tokio + serde.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tracing::{debug, warn};

/// Maximum time to wait for a single MCP request/response round-trip.
const MCP_REQUEST_TIMEOUT_SECS: u64 = 120;

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
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
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
    /// Connect to an MCP server by spawning its process.
    ///
    /// Performs the full initialization handshake:
    /// 1. Spawn process with piped stdin/stdout
    /// 2. Send `initialize` request
    /// 3. Send `notifications/initialized` notification
    /// 4. Send `tools/list` to discover available tools
    pub async fn connect(
        name: &str,
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> Result<Self, anyhow::Error> {
        debug!(
            "mcp: connecting to server '{name}' via: {command} {}",
            args.join(" ")
        );

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("mcp: failed to spawn '{command}': {e}"))?;

        let stdin = BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("mcp: no stdin for '{name}'"))?,
        );
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("mcp: no stdout for '{name}'"))?,
        )
        .lines();

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
            "protocolVersion": "2025-11-25",
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
    ) -> Result<McpToolResult, anyhow::Error> {
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
    async fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, anyhow::Error> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        // Read lines until we get a response with our id, with a timeout guard.
        let server_name = self.server_name.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS),
            Self::read_response(&mut self.stdout, id, &server_name, method),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(anyhow::anyhow!(
                "mcp: '{}' timed out waiting for response to {method} (>{MCP_REQUEST_TIMEOUT_SECS}s)",
                self.server_name
            )),
        }
    }

    /// Read stdout lines until a JSON-RPC response matching `id` arrives.
    async fn read_response(
        stdout: &mut tokio::io::Lines<BufReader<ChildStdout>>,
        id: u64,
        server_name: &str,
        method: &str,
    ) -> Result<Value, anyhow::Error> {
        loop {
            let raw = stdout
                .next_line()
                .await?
                .ok_or_else(|| anyhow::anyhow!("mcp: '{server_name}' stdout closed"))?;

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
                return Err(anyhow::anyhow!(
                    "mcp: '{server_name}' error on {method}: {}",
                    err.message
                ));
            }

            return Ok(resp.result.unwrap_or(Value::Null));
        }
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), anyhow::Error> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: None,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

/// Parse the `tools/list` response into `McpToolDef` items.
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
