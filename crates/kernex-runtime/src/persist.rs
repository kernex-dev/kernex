//! Writing a finished exchange to memory.

use kernex_core::context::ContextNeeds;
use kernex_core::message::{Request, Response};
use kernex_memory::{MemoryError, Store};

impl crate::Runtime {
    /// [`exchange`] into this runtime's store and channel.
    pub(crate) async fn store_exchange(
        &self,
        needs: &ContextNeeds,
        request: &Request,
        response: &Response,
        project: &str,
    ) -> Result<(), MemoryError> {
        exchange(
            needs.store,
            &self.store,
            &self.channel,
            request,
            response,
            project,
        )
        .await
    }
}

/// Store `request` and `response` in `channel`'s conversation for `project`,
/// unless the request's context needs turned storing off (`store` false),
/// in which case memory is left as it was.
pub(crate) async fn exchange(
    store: bool,
    memory: &Store,
    channel: &str,
    request: &Request,
    response: &Response,
    project: &str,
) -> Result<(), MemoryError> {
    if !store {
        return Ok(());
    }
    memory
        .store_exchange(channel, request, response, project)
        .await
}

#[cfg(test)]
mod tests {
    use crate::RuntimeBuilder;
    use kernex_core::context::{Context, ContextNeeds};
    use kernex_core::error::KernexError;
    use kernex_core::message::{CompletionMeta, Request, Response};
    use kernex_core::run::RunConfig;
    use kernex_core::stream::StreamEvent;
    use kernex_core::traits::{Provider, StreamingProvider};

    struct Echo;

    #[async_trait::async_trait]
    impl Provider for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        async fn complete(&self, _ctx: &Context) -> Result<Response, KernexError> {
            Ok(Response {
                text: "noted".to_string(),
                metadata: CompletionMeta::default(),
            })
        }
        async fn is_available(&self) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl StreamingProvider for Echo {
        async fn complete_stream(
            &self,
            _ctx: &Context,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, KernexError> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(StreamEvent::TextDelta("noted".to_string()))
                .await
                .ok();
            tx.send(StreamEvent::Done).await.ok();
            Ok(rx)
        }
    }

    /// Conversations, messages and facts stored for `sender`.
    async fn rows(runtime: &crate::Runtime, sender: &str) -> (i64, i64, i64) {
        let (conversations, messages, _, facts) =
            runtime.store.get_memory_stats(sender).await.unwrap();
        (conversations, messages, facts)
    }

    /// Run a streamed call to its end, then give the task that stores the
    /// exchange time to finish.
    async fn stream(runtime: &crate::Runtime, request: &Request, needs: &ContextNeeds) {
        let mut rx = runtime
            .complete_stream_with_needs(&Echo, request, needs)
            .await
            .unwrap();
        while rx.recv().await.is_some() {}
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn a_call_that_does_not_store_leaves_memory_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = RuntimeBuilder::new()
            .data_dir(dir.path().to_str().unwrap())
            .build()
            .await
            .unwrap();
        let off = ContextNeeds {
            store: false,
            ..ContextNeeds::default()
        };

        // A new sender: nothing is created for it on any path.
        let fresh = Request::text("fresh", "hello there");
        runtime
            .complete_with_needs(&Echo, &fresh, &off)
            .await
            .unwrap();
        stream(&runtime, &fresh, &off).await;
        let config = RunConfig {
            needs: off.clone(),
            ..RunConfig::default()
        };
        runtime.run(&Echo, &fresh, &config).await.unwrap();
        assert_eq!(rows(&runtime, "fresh").await, (0, 0, 0));

        // A sender with a stored conversation: it is read, not extended.
        let known = Request::text("known", "hello there");
        runtime.complete(&Echo, &known).await.unwrap();
        let before = rows(&runtime, "known").await;
        assert_eq!(before.1, 2);
        runtime
            .complete_with_needs(&Echo, &known, &off)
            .await
            .unwrap();
        stream(&runtime, &known, &off).await;
        runtime.run(&Echo, &known, &config).await.unwrap();
        assert_eq!(rows(&runtime, "known").await, before);

        // Storing on: each path adds the exchange.
        stream(&runtime, &known, &ContextNeeds::default()).await;
        runtime
            .run(&Echo, &known, &RunConfig::default())
            .await
            .unwrap();
        assert_eq!(rows(&runtime, "known").await.1, before.1 + 4);
    }
}
