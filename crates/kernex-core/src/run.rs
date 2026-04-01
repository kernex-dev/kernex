//! Configuration and outcome types for the agentic runtime loop.

use crate::message::Response;

/// Selects a performance/cost tier when creating a provider via `ProviderConfig`.
///
/// The factory resolves the concrete model name from the tier and provider type.
/// An explicit `model` string on `ProviderConfig` always takes precedence over tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    /// Cost-efficient model for standard tasks (e.g. Sonnet, GPT-4o-mini, Gemini Flash).
    Standard,
    /// Most capable model for complex tasks (e.g. Opus, GPT-4o, Gemini Pro).
    Flagship,
}

/// Configuration for a [`Runtime::run`] invocation.
///
/// [`Runtime::run`]: crate::traits::Provider
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Maximum number of agentic turns before stopping. Default: 50.
    pub max_turns: u32,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self { max_turns: 50 }
    }
}

/// The terminal outcome of a [`Runtime::run`] call.
#[derive(Debug)]
pub enum RunOutcome {
    /// Provider signaled end-of-turn. Contains the final response.
    EndTurn(Response),
    /// The run hit the `max_turns` limit before the provider stopped.
    MaxTurns,
}
