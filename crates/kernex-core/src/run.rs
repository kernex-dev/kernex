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

/// Configuration for an agentic-loop invocation (consumed by `Runtime::run`
/// in `kernex-runtime`; can't be linked directly from this crate to avoid a
/// circular dependency).
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Maximum number of agentic turns before stopping. Default: 50.
    pub max_turns: u32,
    /// Cumulative billed-token budget for the run. `None` (the default) means
    /// unlimited. When the loop's accumulated billed tokens reach this value,
    /// the provider stops before starting another turn and the run resolves to
    /// [`RunOutcome::BudgetExhausted`]. A completed final answer is always
    /// returned even if it crosses the budget.
    pub token_budget: Option<u64>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            max_turns: 50,
            token_budget: None,
        }
    }
}

/// Returns `true` when `spent` billed tokens have reached or exceeded the
/// configured `budget`. `None` means unlimited (never exhausted).
///
/// Shared by every provider's agentic loop so the budget semantics cannot
/// drift between providers. "Billed" means input + output + cache-creation
/// tokens; cache reads are excluded so well-cached loops are not penalized
/// for the cheap path.
pub fn budget_exhausted(spent: u64, budget: Option<u64>) -> bool {
    budget.is_some_and(|b| spent >= b)
}

/// The terminal outcome of an agentic-loop run.
// `EndTurn` carries a `Response` (the larger variant) while `MaxTurns` is a
// unit. Boxing the `Response` would shrink the enum but change the public
// variant shape (a breaking change for consumers that match `EndTurn(resp)`),
// so we keep it inline; a `RunOutcome` is produced once per run, not in a hot
// path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum RunOutcome {
    /// Provider signaled end-of-turn. Contains the final response.
    EndTurn(Response),
    /// The run hit the `max_turns` limit before the provider stopped.
    MaxTurns,
    /// The run's cumulative billed tokens reached `token_budget` before the
    /// provider produced a final answer.
    BudgetExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_none_is_never_exhausted() {
        assert!(!budget_exhausted(u64::MAX, None));
    }

    #[test]
    fn budget_exhausted_at_and_past_limit() {
        assert!(!budget_exhausted(99, Some(100)));
        assert!(budget_exhausted(100, Some(100)));
        assert!(budget_exhausted(101, Some(100)));
    }

    #[test]
    fn run_config_default_has_no_budget() {
        assert_eq!(RunConfig::default().token_budget, None);
    }
}
