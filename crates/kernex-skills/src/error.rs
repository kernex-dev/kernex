//! Per-crate error type for `kernex-skills`.
//!
//! Replaces the old `kernex_core::KernexError::Skill(String)` shape so
//! callers can pattern-match on the actual cause.

/// Errors produced by skill loading and trigger matching.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// A filesystem operation failed (read SKILL.md, walk skills/, etc).
    #[error("{context}: {source}")]
    Io {
        /// Human-readable description of the failing operation.
        context: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Domain-logic error — invalid skill, parse failure on frontmatter,
    /// missing required fields, etc.
    #[error("{0}")]
    Logic(String),
}

impl SkillError {
    /// Wrap a `std::io::Error` with operation context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Construct a domain-logic error from a message.
    pub fn logic(msg: impl Into<String>) -> Self {
        Self::Logic(msg.into())
    }
}

/// Bridge to the workspace-level aggregate error. Boxes the typed
/// `SkillError` inside `KernexError::Skill` so callers downstream can
/// recover the structured cause via `boxed.downcast_ref::<SkillError>()`.
impl From<SkillError> for kernex_core::error::KernexError {
    fn from(err: SkillError) -> Self {
        kernex_core::error::KernexError::skill(err)
    }
}
