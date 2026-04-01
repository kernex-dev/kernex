//! Configuration and outcome types for the agentic runtime loop.

use crate::message::Response;

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
