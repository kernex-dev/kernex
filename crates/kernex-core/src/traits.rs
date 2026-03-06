//! Core traits that define the Kernex runtime contracts.

use crate::context::Context;
use crate::error::Result;
use crate::message::Response;

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

/// Persistent storage backend. Implement to use a different database.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Initialize the store (run migrations, etc).
    async fn initialize(&self) -> Result<()>;
}
