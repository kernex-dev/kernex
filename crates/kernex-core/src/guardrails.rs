//! Guardrail trait for intercepting and filtering text at the pipeline layer.
//!
//! Guardrails run on input text before it reaches a provider and on output
//! text before it returns to the caller. Each check returns a
//! [`GuardrailAction`] that instructs the runtime to allow, block, or sanitize
//! the text.
//!
//! Wire a guardrail into the runtime via `RuntimeBuilder::guardrail_runner`.

use async_trait::async_trait;

/// What the runtime should do with text after a guardrail check.
#[derive(Debug)]
pub enum GuardrailAction {
    /// Allow the text to proceed unchanged.
    Allow,
    /// Reject the text. The runtime returns [`KernexError::Guardrail`] with the reason.
    ///
    /// [`KernexError::Guardrail`]: crate::error::KernexError::Guardrail
    Block(String),
    /// Replace the text with a sanitized version before it continues.
    Sanitize(String),
}

/// Intercept and filter text at the pipeline layer.
///
/// Implement this trait to add content filtering, PII redaction, prompt
/// injection detection, or compliance auditing. Wire it into the runtime via
/// `RuntimeBuilder::guardrail_runner`.
///
/// Both methods are called synchronously within the request pipeline:
/// `check_input` before the provider call, `check_output` after.
///
/// # Example
///
/// ```rust
/// use async_trait::async_trait;
/// use kernex_core::guardrails::{GuardrailAction, GuardrailRunner};
///
/// struct BlocklistGuardrail {
///     blocked_terms: Vec<&'static str>,
/// }
///
/// #[async_trait]
/// impl GuardrailRunner for BlocklistGuardrail {
///     async fn check_input(&self, text: &str) -> GuardrailAction {
///         for term in &self.blocked_terms {
///             if text.contains(term) {
///                 return GuardrailAction::Block(
///                     format!("blocked term detected: {term}")
///                 );
///             }
///         }
///         GuardrailAction::Allow
///     }
///
///     async fn check_output(&self, _text: &str) -> GuardrailAction {
///         GuardrailAction::Allow
///     }
/// }
/// ```
#[async_trait]
pub trait GuardrailRunner: Send + Sync {
    /// Check input text before it is sent to the provider.
    ///
    /// Return [`GuardrailAction::Block`] to reject the request entirely.
    /// Return [`GuardrailAction::Sanitize`] to replace the text before sending.
    async fn check_input(&self, text: &str) -> GuardrailAction;

    /// Check output text before it is returned to the caller.
    ///
    /// Return [`GuardrailAction::Block`] to surface an error instead of the response.
    /// Return [`GuardrailAction::Sanitize`] to redact the response before returning.
    ///
    /// For streaming responses, this runs on the fully accumulated text after
    /// the stream completes and only affects what is persisted to memory.
    async fn check_output(&self, text: &str) -> GuardrailAction;
}

/// No-op guardrail that allows all text through unchanged.
pub struct NoopGuardrailRunner;

#[async_trait]
impl GuardrailRunner for NoopGuardrailRunner {
    async fn check_input(&self, _text: &str) -> GuardrailAction {
        GuardrailAction::Allow
    }

    async fn check_output(&self, _text: &str) -> GuardrailAction {
        GuardrailAction::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_allows_input() {
        let runner = NoopGuardrailRunner;
        let action = runner.check_input("some input text").await;
        assert!(matches!(action, GuardrailAction::Allow));
    }

    #[tokio::test]
    async fn test_noop_allows_output() {
        let runner = NoopGuardrailRunner;
        let action = runner.check_output("some output text").await;
        assert!(matches!(action, GuardrailAction::Allow));
    }

    #[tokio::test]
    async fn test_custom_block_guardrail() {
        struct BlockAll;
        #[async_trait]
        impl GuardrailRunner for BlockAll {
            async fn check_input(&self, _text: &str) -> GuardrailAction {
                GuardrailAction::Block("blocked".to_string())
            }
            async fn check_output(&self, _text: &str) -> GuardrailAction {
                GuardrailAction::Allow
            }
        }

        let runner = BlockAll;
        let action = runner.check_input("anything").await;
        assert!(matches!(action, GuardrailAction::Block(_)));
        if let GuardrailAction::Block(reason) = action {
            assert_eq!(reason, "blocked");
        }
    }

    #[tokio::test]
    async fn test_custom_sanitize_guardrail() {
        struct RedactPii;
        #[async_trait]
        impl GuardrailRunner for RedactPii {
            async fn check_input(&self, text: &str) -> GuardrailAction {
                GuardrailAction::Sanitize(text.replace("secret", "[REDACTED]"))
            }
            async fn check_output(&self, _text: &str) -> GuardrailAction {
                GuardrailAction::Allow
            }
        }

        let runner = RedactPii;
        let action = runner.check_input("my secret is 42").await;
        assert!(matches!(action, GuardrailAction::Sanitize(_)));
        if let GuardrailAction::Sanitize(clean) = action {
            assert_eq!(clean, "my [REDACTED] is 42");
        }
    }
}
