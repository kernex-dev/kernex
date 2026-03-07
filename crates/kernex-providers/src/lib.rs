//! kernex-providers: AI backend implementations and tool execution.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides 6 AI providers (Claude Code CLI, Anthropic, OpenAI, Ollama,
//! OpenRouter, Gemini), a shared tool executor with sandbox enforcement,
//! and an MCP client for external tool integration.

pub mod anthropic;
pub mod claude_code;
pub mod factory;
pub mod gemini;
pub mod http_retry;
pub(crate) mod mcp_client;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub(crate) mod tools;
