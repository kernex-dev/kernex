//! Errors of the MCP client.

use thiserror::Error;

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
        "mcp: '{server}' not started: command {command:?} is a relative path or is not in a declared command allow-list"
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
    /// Round-trip exceeded [`MCP_REQUEST_TIMEOUT_SECS`](crate::mcp_client::MCP_REQUEST_TIMEOUT_SECS).
    #[error("mcp: '{server}' timed out waiting for response to {method} (>{seconds}s)")]
    Timeout {
        server: String,
        method: String,
        seconds: u64,
    },
}
