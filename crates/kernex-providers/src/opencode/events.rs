//! Parse the JSON event stream of `opencode run --format json`.
//!
//! One JSON object per line. Each carries `type`, `timestamp`, `sessionID`
//! and usually a `part`. The ones kernex reads:
//!
//! - `step_start`: opens a new step (one model turn).
//! - `text`: assistant text in `part.text`.
//! - `step_finish`: `part.reason` and `part.tokens` (`total`, `input`,
//!   `output`, ...).
//! - `error`: a run-level failure in `error`.
//!
//! Unknown event types and non-JSON lines are skipped.

use serde_json::Value;

/// What kernex keeps from one OpenCode run.
#[derive(Debug, Default, PartialEq)]
pub(super) struct RunOutput {
    /// Assistant text of the last step that produced any, parts joined with
    /// blank lines. Earlier steps hold tool-call narration ("I'll read the
    /// file..."), which is left out.
    pub text: String,
    /// OpenCode session ID, for `-s` continuation.
    pub session_id: Option<String>,
    /// Tokens summed over every step, when OpenCode reported them.
    pub tokens_used: Option<u64>,
    /// `reason` of the last finished step (`stop`, `tool-calls`, ...).
    pub stop_reason: Option<String>,
    /// First error message reported by OpenCode, if any.
    pub error: Option<String>,
}

/// Token count of one `step_finish` event.
fn step_tokens(part: &Value) -> Option<u64> {
    let tokens = part.get("tokens")?;
    if let Some(total) = tokens.get("total").and_then(Value::as_u64) {
        return Some(total);
    }
    let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
    let output = tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
    let reasoning = tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0);
    Some(input + output + reasoning)
}

/// Best-effort human-readable message from an `error` event.
fn error_message(event: &Value) -> String {
    let err = event.get("error").unwrap_or(event);
    let msg = err
        .pointer("/data/message")
        .or_else(|| err.get("message"))
        .and_then(Value::as_str);
    match msg {
        Some(m) => m.to_string(),
        None => err.to_string(),
    }
}

/// Parse the full stdout of one run.
pub(super) fn parse_events(stdout: &str) -> RunOutput {
    let mut out = RunOutput::default();
    // Text parts grouped per step; a new group opens on each `step_start`.
    let mut steps: Vec<Vec<String>> = vec![Vec::new()];

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if out.session_id.is_none() {
            out.session_id = event
                .get("sessionID")
                .and_then(Value::as_str)
                .map(String::from);
        }
        let part = event.get("part").cloned().unwrap_or(Value::Null);
        match event.get("type").and_then(Value::as_str) {
            Some("step_start") => {
                if steps.last().is_some_and(|s| !s.is_empty()) {
                    steps.push(Vec::new());
                }
            }
            Some("text") => {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    let t = t.trim();
                    if !t.is_empty() {
                        if let Some(step) = steps.last_mut() {
                            step.push(t.to_string());
                        }
                    }
                }
            }
            Some("step_finish") => {
                if let Some(n) = step_tokens(&part) {
                    out.tokens_used = Some(out.tokens_used.unwrap_or(0) + n);
                }
                if let Some(r) = part.get("reason").and_then(Value::as_str) {
                    out.stop_reason = Some(r.to_string());
                }
            }
            Some("error") if out.error.is_none() => {
                out.error = Some(error_message(&event));
            }
            _ => {}
        }
    }

    if let Some(step) = steps.iter().rev().find(|s| !s.is_empty()) {
        out.text = step.join("\n\n");
    }
    out
}
