//! Hook system for intercepting tool execution lifecycle events.

use async_trait::async_trait;
use serde_json::Value;

/// The outcome of a pre-tool hook check.
#[derive(Debug)]
pub enum HookOutcome {
    /// Allow the tool to execute.
    Allow,
    /// Block the tool with a reason message.
    Blocked(String),
}

/// Lifecycle hooks for tool execution.
///
/// Implement this trait to intercept tool calls for logging, approval
/// workflows, rate limiting, or auditing. Wire into [`Context`] via
/// [`Context::with_hooks`].
///
/// [`Context`]: crate::context::Context
#[async_trait]
pub trait HookRunner: Send + Sync + std::fmt::Debug {
    /// Called before a tool executes. Return [`HookOutcome::Blocked`] to cancel it.
    async fn pre_tool(&self, tool_name: &str, input: &Value) -> HookOutcome;
    /// Called after a tool completes. Blocked tools do not fire this.
    async fn post_tool(&self, tool_name: &str, result: &str, is_error: bool);
    /// Called when the provider signals end-of-turn.
    async fn on_stop(&self, final_text: &str);
}

/// Default no-op implementation that allows all tools to execute.
#[derive(Debug)]
pub struct NoopHookRunner;

#[async_trait]
impl HookRunner for NoopHookRunner {
    async fn pre_tool(&self, _tool_name: &str, _input: &Value) -> HookOutcome {
        HookOutcome::Allow
    }

    async fn post_tool(&self, _tool_name: &str, _result: &str, _is_error: bool) {}

    async fn on_stop(&self, _final_text: &str) {}
}
