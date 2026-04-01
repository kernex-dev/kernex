//! Core traits that define the Kernex runtime contracts.

use crate::context::Context;
use crate::error::Result;
use crate::message::Response;
use crate::stream::StreamEvent;

/// AI backend provider. Implement to add a new model/service.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Whether this provider requires an API key to function.
    fn requires_api_key(&self) -> bool;

    /// Send a conversation context to the provider and get a response.
    async fn complete(&self, context: &Context) -> Result<Response>;

    /// Check if the provider is available and ready.
    async fn is_available(&self) -> bool;
}

/// Extension of [`Provider`] for SSE-based real-time streaming responses.
///
/// Implement this trait alongside `Provider` to expose streaming on a backend
/// that supports it (e.g. Anthropic, OpenAI). Consumers can check for this
/// trait via `as_any` downcasting or explicit provider selection.
///
/// The returned channel receives [`StreamEvent`] variants until a [`StreamEvent::Done`]
/// or [`StreamEvent::Error`] is sent, after which the sender is dropped.
#[async_trait::async_trait]
pub trait StreamingProvider: Provider {
    /// Send a context to the provider and stream delta events via a channel.
    ///
    /// Returns a receiver that yields events as they arrive. The task sending
    /// events is detached; dropping the receiver cancels the stream.
    async fn complete_stream(
        &self,
        context: &Context,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>>;
}

/// Provides text summarization. Used for context auto-compact.
///
/// Implement this trait on any type that can summarize a block of text
/// (e.g. a `Provider` wrapper). Inject it into `build_context` to enable
/// the [`CompactionStrategy::Summarize`](crate::context::CompactionStrategy) strategy.
#[async_trait::async_trait]
pub trait Summarizer: Send + Sync {
    /// Summarize `text` into a shorter form.
    async fn summarize(&self, text: &str) -> Result<String>;
}

/// Persistent storage backend. Implement to use a different database.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Initialize the store (run migrations, etc).
    async fn initialize(&self) -> Result<()>;
}
