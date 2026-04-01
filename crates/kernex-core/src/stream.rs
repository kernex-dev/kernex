//! Streaming types for SSE-based provider responses.

/// An event emitted by a streaming provider.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text delta from the model.
    TextDelta(String),
    /// A JSON fragment from a tool input being streamed.
    InputJsonDelta(String),
    /// Streaming is complete.
    Done,
    /// A streaming-level error.
    Error(String),
}

/// Accumulates [`StreamEvent`] deltas into a complete text response.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    text: String,
}

impl StreamAccumulator {
    /// Create a new accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a streaming event into the accumulator.
    pub fn push(&mut self, event: &StreamEvent) {
        if let StreamEvent::TextDelta(delta) = event {
            self.text.push_str(delta);
        }
    }

    /// Returns the accumulated text so far.
    pub fn text(&self) -> &str {
        &self.text
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
}
