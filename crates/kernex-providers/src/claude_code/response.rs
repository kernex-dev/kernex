//! CLI response parsing and diagnostics.

use super::{ClaudeCliResponse, ClaudeCodeProvider};
use tracing::{debug, error, warn};

impl ClaudeCodeProvider {
    /// Parse the JSON response from Claude Code CLI, with diagnostic logging.
    pub(super) fn parse_response(
        &self,
        stdout: &str,
        effective_max_turns: u32,
    ) -> (String, Option<String>) {
        match serde_json::from_str::<ClaudeCliResponse>(stdout) {
            Ok(resp) => {
                // Handle error_max_turns — still extract whatever result exists.
                if resp.subtype.as_deref() == Some("error_max_turns") {
                    warn!(
                        "claude hit max_turns limit | num_turns={:?} effective_limit={} configured={}",
                        resp.num_turns, effective_max_turns, self.max_turns
                    );
                }

                let text = match resp.result {
                    Some(ref r) if !r.is_empty() => r.clone(),
                    _ => {
                        // Log the full response for debugging empty results.
                        error!(
                            "claude returned empty result | is_error={} subtype={:?} \
                             type={:?} num_turns={:?} | raw_len={}",
                            resp.is_error,
                            resp.subtype,
                            resp.response_type,
                            resp.num_turns,
                            stdout.len(),
                        );
                        debug!("full claude response for empty result: {stdout}");

                        if resp.is_error {
                            format!(
                                "Error from Claude: {}",
                                resp.subtype.as_deref().unwrap_or("unknown")
                            )
                        } else {
                            // Try to provide something useful instead of a generic fallback.
                            "I received your message but wasn't able to generate a response. \
                             Please try again."
                                .to_string()
                        }
                    }
                };
                (text, resp.model)
            }
            Err(e) => {
                // JSON parsing failed — try to extract text from raw output.
                warn!("failed to parse claude JSON response: {e}");
                debug!(
                    "raw stdout (first 500 chars): {}",
                    &stdout[..stdout.len().min(500)]
                );

                let trimmed = stdout.trim();
                if trimmed.is_empty() {
                    (
                        "I received your message but the response was empty. Please try again."
                            .to_string(),
                        None,
                    )
                } else {
                    (trimmed.to_string(), None)
                }
            }
        }
    }
}
