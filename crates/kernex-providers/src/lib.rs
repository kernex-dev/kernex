//! kernex-providers: AI backend implementations and tool execution.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides 7 AI providers (Claude Code CLI, Anthropic, OpenAI, Ollama,
//! OpenRouter, Gemini, AWS Bedrock), a shared tool executor with sandbox
//! enforcement, and an MCP client for external tool integration.

pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
pub mod claude_code;
pub mod factory;
pub mod gemini;
pub mod http_retry;
pub(crate) mod mcp_client;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod tool_params;
pub(crate) mod tools;
