#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! kernex-providers: AI backend implementations and tool execution.
//!
//! Provides 12 AI providers (Claude Code CLI, OpenCode CLI, Anthropic, OpenAI,
//! Ollama, OpenRouter, Gemini, Groq, Mistral, DeepSeek, Fireworks, xAI) plus
//! AWS Bedrock behind the `bedrock` feature, a shared tool executor with sandbox
//! enforcement, and an MCP client for external tool integration.
//!
//! Tool calls are scoped to the workspace and the runtime directories in
//! [`SandboxDirs`] (or the [`factory::ProviderConfig`] fields of the same
//! names):
//!
//! - `bash`, toolbox tools and MCP servers run inside the OS sandbox of
//!   `kernex-sandbox`. An MCP server starts only when its command is listed
//!   in its skill's `permissions.commands`, and has no network unless the
//!   skill grants it (`permissions.network`, or `network = true` on its
//!   `[mcp.<name>]` frontmatter entry) or the host profile already allows
//!   it.
//! - The in-process file tools (`read`, `write`, `edit`, `grep`, `glob`)
//!   reach the workspace and the profile's `allowed_paths`, plus, for
//!   reads, `read_paths` and the data directory's `skills/` and
//!   `projects/`, and nothing else.
//! - `web_fetch` is off until the host sets a [`WebFetchPolicy`] other than
//!   `Disabled`.
//! - `allowed_tools` offers exactly the tools it names; `None` offers all.
//! - The `claude-code` and `opencode` providers always run their CLI inside
//!   the OS sandbox, with its state in `cli/` under the data directory and
//!   its permission bypass off unless `cli_permission_bypass` is set.
//!
//! SECURITY.md in the repository lists the limits of each layer.

pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
pub mod claude_code;
#[cfg(test)]
mod cli_e2e_tests;
#[cfg(test)]
mod cli_probe_tests;
pub(crate) mod cli_support;
pub mod error;
pub mod factory;
pub mod gemini;
pub mod http_retry;
pub(crate) mod mcp_client;
pub(crate) mod mcp_error;
pub mod ollama;
pub mod openai;
pub mod opencode;
pub mod openrouter;
mod sandbox_dirs;
pub mod tool_params;
pub(crate) mod tools;

pub use error::ProviderError;
pub use sandbox_dirs::SandboxDirs;
pub use tools::WebFetchPolicy;
