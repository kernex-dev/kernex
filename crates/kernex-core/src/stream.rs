//! Streaming types for SSE-based provider responses.

/// Token usage and stop reason reported over a stream.
///
/// Providers emit this near the end of a stream (for Anthropic, assembled from
/// the `message_start` and `message_delta` SSE events). It lets the caller
/// record cost and detect why the turn ended without buffering the whole reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamUsage {
    /// Prompt (input) tokens billed.
    pub input_tokens: u64,
    /// Completion (output) tokens billed.
    pub output_tokens: u64,
    /// Tokens served from the prompt cache, if reported.
    pub cache_read_tokens: Option<u64>,
    /// Tokens written into the prompt cache, if reported.
    pub cache_creation_tokens: Option<u64>,
    /// Why the turn ended (`end_turn` / `max_tokens` / `tool_use` / ...).
    pub stop_reason: Option<String>,
}

impl StreamUsage {
    /// Total billed tokens across every dimension (input + output + cache).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens.unwrap_or(0)
            + self.cache_creation_tokens.unwrap_or(0)
    }
}

/// An event emitted by a streaming provider.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text delta from the model.
    TextDelta(String),
    /// A thinking (chain-of-thought) text delta. Emitted only when thinking is
    /// enabled and the model streams reasoning; it is not part of the answer.
    ThinkingDelta(String),
    /// A JSON fragment from a tool input being streamed.
    InputJsonDelta(String),
    /// Token usage and stop reason for the turn. Emitted once, just before
    /// [`StreamEvent::Done`].
    Usage(StreamUsage),
    /// Streaming is complete.
    Done,
    /// A streaming-level error (transport failure or an SSE `error` event such
    /// as `overloaded_error`). Terminal: nothing valid follows.
    Error(String),
}

/// Accumulates [`StreamEvent`] deltas into a complete text response plus the
/// final usage.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    text: String,
    usage: Option<StreamUsage>,
}

impl StreamAccumulator {
    /// Create a new accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a streaming event into the accumulator.
    pub fn push(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta(delta) => self.text.push_str(delta),
            StreamEvent::Usage(usage) => self.usage = Some(usage.clone()),
            // Thinking deltas are reasoning, not the answer; they are forwarded
            // to the caller but not folded into the persisted text.
            StreamEvent::ThinkingDelta(_)
            | StreamEvent::InputJsonDelta(_)
            | StreamEvent::Done
            | StreamEvent::Error(_) => {}
        }
    }

    /// Returns the accumulated text so far.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The final usage reported on the stream, if any was seen.
    pub fn usage(&self) -> Option<&StreamUsage> {
        self.usage.as_ref()
    }

    /// Total billed tokens reported on the stream, if usage was seen.
    pub fn total_tokens(&self) -> Option<u64> {
        self.usage.as_ref().map(StreamUsage::total_tokens)
    }

    /// Consumes the accumulator and returns the final text.
    pub fn into_text(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_text_deltas() {
        let mut acc = StreamAccumulator::new();
        acc.push(&StreamEvent::TextDelta("Hello".to_string()));
        acc.push(&StreamEvent::TextDelta(", world".to_string()));
        acc.push(&StreamEvent::Done);
        assert_eq!(acc.text(), "Hello, world");
    }

    #[test]
    fn ignores_non_text_events() {
        let mut acc = StreamAccumulator::new();
        acc.push(&StreamEvent::InputJsonDelta("{\"foo\":".to_string()));
        acc.push(&StreamEvent::Done);
        assert_eq!(acc.text(), "");
    }

    #[test]
    fn into_text_consumes() {
        let mut acc = StreamAccumulator::new();
        acc.push(&StreamEvent::TextDelta("hi".to_string()));
        assert_eq!(acc.into_text(), "hi");
    }

    #[test]
    fn captures_usage_and_excludes_thinking_from_text() {
        let mut acc = StreamAccumulator::new();
        acc.push(&StreamEvent::ThinkingDelta("reasoning...".to_string()));
        acc.push(&StreamEvent::TextDelta("answer".to_string()));
        acc.push(&StreamEvent::Usage(StreamUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: Some(100),
            cache_creation_tokens: None,
            stop_reason: Some("end_turn".to_string()),
        }));
        acc.push(&StreamEvent::Done);
        // Thinking is not part of the answer text.
        assert_eq!(acc.text(), "answer");
        assert_eq!(acc.total_tokens(), Some(115));
        assert_eq!(
            acc.usage().unwrap().stop_reason.as_deref(),
            Some("end_turn")
        );
    }

    #[test]
    fn total_tokens_none_without_usage() {
        let mut acc = StreamAccumulator::new();
        acc.push(&StreamEvent::TextDelta("hi".to_string()));
        assert_eq!(acc.total_tokens(), None);
    }
}
