//! OpenAI's Responses API (`POST {base_url}/responses`): the request and
//! response types and the agentic loop for the `openai` provider.
//!
//! GPT-6.2 Sol and Astra, GPT-6.1 Sol and Terra, and GPT-6 Astra call tools
//! only through this API. The loop runs stateless: every request sets
//! `store: false` and carries the whole conversation, and each turn's
//! output items (reasoning included, with its encrypted content) go back to
//! the model unchanged, as OpenAI requires for reasoning models that call
//! tools.

use kernex_core::{context::ApiMessage, error::KernexError, message::Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;
use tracing::{debug, info};

use crate::error::ProviderError;
use crate::http_retry::{read_truncated_error_body, send_with_retry};
use crate::openai_wire::is_reasoning_model;
use crate::tools::{build_response, ToolDef, ToolExecutor};

/// Reasoning effort for every request of a run. GPT-6.2 Sol and Astra
/// reject `none`, and `medium`, the API default, spends more reasoning
/// tokens on each tool turn.
const REASONING_EFFORT: &str = "low";

/// The encrypted reasoning in each reasoning item, which is what lets it be
/// passed back while nothing is stored on OpenAI's side. Stateless requests
/// get it by default; asking for it keeps that explicit.
const INCLUDE: [&str; 1] = ["reasoning.encrypted_content"];

#[derive(Serialize)]
pub(crate) struct ResponsesRequest<'a> {
    pub model: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub instructions: &'a str,
    pub input: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<&'a [ResponsesTool]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<[&'static str; 1]>,
}

#[derive(Serialize)]
pub(crate) struct Reasoning {
    pub effort: &'static str,
}

/// A function tool. Responses takes the function's fields flat, where Chat
/// Completions nests them under `function`.
#[derive(Serialize, Clone, Debug)]
pub(crate) struct ResponsesTool {
    #[serde(rename = "type")]
    pub tool_type: &'static str,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Left out, Responses rewrites a schema into strict mode where it can,
    /// which changes what the model must fill in. `false` keeps the schema
    /// as the tool declares it, as on Chat Completions.
    pub strict: bool,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesResponse {
    #[serde(default)]
    pub output: Vec<Value>,
    pub model: Option<String>,
    pub usage: Option<ResponsesUsage>,
    pub status: Option<String>,
    pub incomplete_details: Option<IncompleteDetails>,
    pub error: Option<ResponseError>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesUsage {
    pub total_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl ResponsesUsage {
    /// The billed total; input plus output when a server leaves the total
    /// out, so the budget still counts.
    fn total(&self) -> Option<u64> {
        self.total_tokens
            .or(match (self.input_tokens, self.output_tokens) {
                (None, None) => None,
                (i, o) => Some(i.unwrap_or(0).saturating_add(o.unwrap_or(0))),
            })
    }
}

/// A function call's arguments as JSON. Empty arguments are an empty
/// object, the form a function without parameters takes.
fn call_arguments(arguments: &str) -> Result<Value, serde_json::Error> {
    if arguments.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(arguments)
    }
}

#[derive(Deserialize)]
pub(crate) struct IncompleteDetails {
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ResponseError {
    pub code: Option<String>,
    pub message: Option<String>,
}

/// Only a `completed` response (or one without a status) is used. Any
/// other status, `incomplete` included, is an error that says why: a cut-off
/// answer would pass for a whole one, and a cut-off call would run a tool
/// on arguments the model did not finish.
fn check_status(resp: &ResponsesResponse, provider_name: &str) -> Result<(), KernexError> {
    let status = match resp.status.as_deref() {
        None | Some("completed") => return Ok(()),
        Some(status) => status,
    };
    let why = if status == "incomplete" {
        resp.incomplete_details
            .as_ref()
            .and_then(|d| d.reason.clone())
    } else {
        resp.error.as_ref().map(|e| {
            [e.code.as_deref(), e.message.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(": ")
        })
    }
    .filter(|why| !why.is_empty())
    .unwrap_or_else(|| "no reason given".to_string());
    Err(ProviderError::Logic(format!("{provider_name}: the response is {status} ({why})")).into())
}

/// The output items the loop reads. Everything else, reasoning included,
/// is passed back as returned.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum OutputItem {
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        content: Vec<ContentPart>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentPart {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "refusal")]
    Refusal { refusal: String },
    #[serde(other)]
    Other,
}

/// What one response asks for: the function calls to run, and the text of
/// its messages (refusals only when there is no other text).
struct Turn {
    calls: Vec<(String, String, String)>,
    text: String,
}

fn read_turn(output: &[Value], provider_name: &str) -> Result<Turn, KernexError> {
    let mut calls = Vec::new();
    let mut text = String::new();
    let mut refusal = String::new();
    for item in output {
        let item = OutputItem::deserialize(item).map_err(|e| {
            ProviderError::Logic(format!("{provider_name}: unreadable output item: {e}"))
        })?;
        match item {
            OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => calls.push((call_id, name, arguments)),
            OutputItem::Message { content } => {
                for part in content {
                    match part {
                        ContentPart::OutputText { text: t } => text.push_str(&t),
                        ContentPart::Refusal { refusal: r } => refusal.push_str(&r),
                        ContentPart::Other => {}
                    }
                }
            }
            OutputItem::Other => {}
        }
    }
    if text.is_empty() {
        text = refusal;
    }
    Ok(Turn { calls, text })
}

/// The conversation as Responses input items. The system prompt is sent as
/// `instructions`, not as an item.
pub(crate) fn build_input(api_messages: &[ApiMessage]) -> Vec<Value> {
    api_messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.content}))
        .collect()
}

pub(crate) fn to_responses_tools(defs: &[ToolDef]) -> Vec<ResponsesTool> {
    defs.iter()
        .map(|d| ResponsesTool {
            tool_type: "function",
            name: d.name.clone(),
            description: d.description.clone(),
            parameters: d.parameters.clone(),
            strict: false,
        })
        .collect()
}

/// Send one request and read the response.
async fn post(
    client: &reqwest::Client,
    url: &str,
    auth_header: &str,
    body: &ResponsesRequest<'_>,
    provider_name: &str,
) -> Result<ResponsesResponse, KernexError> {
    let body_json = serde_json::to_vec(body)
        .map_err(|e| ProviderError::Logic(format!("{provider_name}: serialize failed: {e}")))?;
    let resp = send_with_retry(provider_name, || {
        let req = client
            .post(url)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(body_json.clone());
        async move { req.send().await }
    })
    .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = read_truncated_error_body(resp).await;
        return Err(
            ProviderError::Logic(format!("{provider_name} returned {status}: {text}")).into(),
        );
    }
    Ok(resp.json().await.map_err(|e| {
        ProviderError::Logic(format!("{provider_name}: failed to parse response: {e}"))
    })?)
}

/// The request for one turn: always stateless. A reasoning model gets the
/// run's effort and its encrypted reasoning back; any other model, such as
/// `gpt-4o`, rejects both fields, so they are left out.
fn request<'a>(
    model: &'a str,
    system: &'a str,
    input: &'a [Value],
    tools: Option<&'a [ResponsesTool]>,
) -> ResponsesRequest<'a> {
    let reasons = is_reasoning_model(model);
    ResponsesRequest {
        model,
        instructions: system,
        input,
        tools,
        reasoning: reasons.then_some(Reasoning {
            effort: REASONING_EFFORT,
        }),
        store: false,
        include: reasons.then_some(INCLUDE),
    }
}

/// One request without tools: the text of the answer. A function call in
/// the answer is ignored, since no tool was offered.
pub(crate) async fn responses_complete(
    client: &reqwest::Client,
    url: &str,
    auth_header: &str,
    model: &str,
    system: &str,
    api_messages: &[ApiMessage],
    provider_name: &str,
) -> Result<Response, KernexError> {
    let start = Instant::now();
    let input = build_input(api_messages);
    let body = request(model, system, &input, None);
    debug!("{provider_name}: POST {url} model={model} (no tools)");
    let parsed = post(client, url, auth_header, &body, provider_name).await?;
    check_status(&parsed, provider_name)?;
    let text = read_turn(&parsed.output, provider_name)?.text;
    let text = if text.is_empty() {
        format!("No response from {provider_name}.")
    } else {
        text
    };
    let tokens = parsed.usage.and_then(|u| u.total()).unwrap_or(0);
    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(build_response(
        text,
        provider_name,
        tokens,
        elapsed_ms,
        parsed.model,
    ))
}

/// Run the Responses agentic loop: infer, run the function calls, send the
/// results back, until a response has no function call or `max_turns` is
/// reached. Same contract as `openai_agentic_complete`: a final text, or an
/// empty text with `stop_reason` `budget_exhausted` or `max_turns`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn responses_agentic_complete(
    client: &reqwest::Client,
    url: &str,
    auth_header: &str,
    model: &str,
    system: &str,
    api_messages: &[ApiMessage],
    executor: &mut ToolExecutor,
    max_turns: u32,
    token_budget: Option<u64>,
    provider_name: &str,
) -> Result<Response, KernexError> {
    let start = Instant::now();
    let mut input = build_input(api_messages);
    let all_tool_defs = executor.all_tool_defs();
    let tools = (!all_tool_defs.is_empty()).then(|| to_responses_tools(&all_tool_defs));

    let mut last_model: Option<String> = None;
    let mut total_tokens: u64 = 0;

    for turn in 0..max_turns {
        let body = request(model, system, &input, tools.as_deref());
        debug!("{provider_name}: POST {url} model={model} turn={turn}");
        let parsed = post(client, url, auth_header, &body, provider_name).await?;

        check_status(&parsed, provider_name)?;
        if let Some(m) = parsed.model {
            last_model = Some(m);
        }
        if let Some(t) = parsed.usage.and_then(|u| u.total()) {
            total_tokens = total_tokens.saturating_add(t);
        }

        let Turn { calls, text } = read_turn(&parsed.output, provider_name)?;
        if calls.is_empty() {
            let text = if text.is_empty() {
                format!("No response from {provider_name}.")
            } else {
                text
            };
            let elapsed_ms = start.elapsed().as_millis() as u64;
            return Ok(build_response(
                text,
                provider_name,
                total_tokens,
                elapsed_ms,
                last_model,
            ));
        }

        // Every item of this turn goes back first, in order, then one
        // output per call.
        input.extend(parsed.output);
        for (call_id, name, arguments) in calls {
            // A tool is not run on arguments that do not parse; the model
            // gets the reason as the call's output instead.
            let output = match call_arguments(&arguments) {
                Ok(args) => {
                    info!("{provider_name}: tool call [{turn}] {name} ({call_id})");
                    executor.execute(&name, &args).await.content
                }
                Err(e) => format!(
                    "Error: the arguments for {name} are not valid JSON ({e}); the tool was not run"
                ),
            };
            input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
        }

        if kernex_core::run::budget_exhausted(total_tokens, token_budget) {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let mut resp = build_response(
                String::new(),
                provider_name,
                total_tokens,
                elapsed_ms,
                last_model,
            );
            resp.metadata.stop_reason = Some("budget_exhausted".to_string());
            return Ok(resp);
        }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let mut resp = build_response(
        String::new(),
        provider_name,
        total_tokens,
        elapsed_ms,
        last_model,
    );
    resp.metadata.stop_reason = Some("max_turns".to_string());
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A turn with a reasoning item and two function calls, in the shapes
    /// OpenAI documents for `store: false`. To be replaced with a body
    /// captured from the live run.
    const TURN_WITH_CALLS: &str = r#"{
        "id": "resp_1", "object": "response", "status": "completed",
        "model": "gpt-6.2-sol-2026-11-01",
        "output": [
            {"id": "rs_1", "type": "reasoning", "summary": [],
             "encrypted_content": "gAAAAopaque-reasoning-1"},
            {"id": "fc_1", "type": "function_call", "status": "completed",
             "call_id": "call_a", "name": "read",
             "arguments": "{\"file_path\":\"a.txt\"}"},
            {"id": "fc_2", "type": "function_call", "status": "completed",
             "call_id": "call_b", "name": "read",
             "arguments": "{\"file_path\":\"b.txt\"}"}
        ],
        "usage": {"input_tokens": 90, "output_tokens": 30,
                  "output_tokens_details": {"reasoning_tokens": 20},
                  "total_tokens": 120}
    }"#;

    /// The final answer after the function call outputs.
    const FINAL_MESSAGE: &str = r#"{
        "id": "resp_2", "object": "response", "status": "completed",
        "model": "gpt-6.2-sol-2026-11-01",
        "output": [
            {"id": "rs_2", "type": "reasoning", "summary": [],
             "encrypted_content": "gAAAAopaque-reasoning-2"},
            {"id": "msg_1", "type": "message", "status": "completed",
             "role": "assistant",
             "content": [{"type": "output_text", "annotations": [],
                          "text": "a.txt says alpha; b.txt says beta."}]}
        ],
        "usage": {"input_tokens": 150, "output_tokens": 30, "total_tokens": 180}
    }"#;

    type Bodies = Arc<Mutex<Vec<Value>>>;

    /// A Responses stub that answers the requests in order with `replies`
    /// (the last one repeats) and records each request body.
    async fn spawn_stub(replies: Vec<(u16, String)>) -> (String, Bodies) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let bodies: Bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = bodies.clone();
        tokio::spawn(async move {
            let mut next = 0usize;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (status, reply) = replies[next.min(replies.len() - 1)].clone();
                next += 1;
                let mut buf = vec![0u8; 262_144];
                let mut read = 0usize;
                let mut body_start = 0usize;
                loop {
                    let n = sock.read(&mut buf[read..]).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    read += n;
                    let s = String::from_utf8_lossy(&buf[..read]);
                    if let Some(h_end) = s.find("\r\n\r\n") {
                        body_start = h_end + 4;
                        let cl = s.lines().find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        });
                        if read - body_start >= cl.unwrap_or(0) {
                            break;
                        }
                    }
                }
                if let Ok(v) = serde_json::from_slice(&buf[body_start..read]) {
                    seen.lock().unwrap().push(v);
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (url, bodies)
    }

    fn ok(body: &str) -> (u16, String) {
        (200, body.to_string())
    }

    fn workspace() -> (tempfile::TempDir, ToolExecutor) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        std::fs::write(ws.join("a.txt"), "alpha").unwrap();
        std::fs::write(ws.join("b.txt"), "beta").unwrap();
        let executor = ToolExecutor::new(ws);
        (dir, executor)
    }

    fn user(text: &str) -> Vec<ApiMessage> {
        vec![ApiMessage {
            role: "user".into(),
            content: text.into(),
        }]
    }

    async fn run(
        url: &str,
        executor: &mut ToolExecutor,
        max_turns: u32,
        token_budget: Option<u64>,
    ) -> Result<Response, KernexError> {
        responses_agentic_complete(
            &reqwest::Client::new(),
            url,
            "Bearer k",
            "gpt-6.2-sol",
            "Be brief.",
            &user("What do a.txt and b.txt say?"),
            executor,
            max_turns,
            token_budget,
            "openai",
        )
        .await
    }

    #[tokio::test]
    async fn a_tool_turn_round_trips_its_items_unchanged() {
        let (url, bodies) = spawn_stub(vec![ok(TURN_WITH_CALLS), ok(FINAL_MESSAGE)]).await;
        let (_dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 10, None).await.unwrap();

        assert_eq!(resp.text, "a.txt says alpha; b.txt says beta.");
        assert_eq!(resp.metadata.tokens_used, Some(300));
        assert_eq!(
            resp.metadata.model.as_deref(),
            Some("gpt-6.2-sol-2026-11-01")
        );
        assert_eq!(resp.metadata.stop_reason, None);

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        for body in bodies.iter() {
            assert_eq!(body["model"], "gpt-6.2-sol");
            assert_eq!(body["instructions"], "Be brief.");
            assert_eq!(body["store"], false);
            assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
            assert_eq!(body["reasoning"], json!({"effort": "low"}));
            assert!(body.get("previous_response_id").is_none());
            let tools = body["tools"].as_array().unwrap();
            assert!(!tools.is_empty());
            for tool in tools {
                assert_eq!(tool["type"], "function");
                assert_eq!(tool["strict"], false);
                assert!(tool["name"].is_string() && tool["parameters"].is_object());
                assert!(tool.get("function").is_none());
            }
        }
        assert_eq!(
            bodies[0]["input"],
            json!([{"role": "user", "content": "What do a.txt and b.txt say?"}])
        );

        // The second request carries the first turn's items as returned,
        // reasoning first, then one output per call in call order.
        let input = bodies[1]["input"].as_array().unwrap();
        let first: Value = serde_json::from_str(TURN_WITH_CALLS).unwrap();
        let returned = first["output"].as_array().unwrap();
        assert_eq!(input.len(), 1 + returned.len() + 2);
        assert_eq!(&input[1..1 + returned.len()], returned.as_slice());
        let outputs = &input[1 + returned.len()..];
        assert_eq!(outputs[0]["type"], "function_call_output");
        assert_eq!(outputs[0]["call_id"], "call_a");
        assert!(outputs[0]["output"].as_str().unwrap().contains("alpha"));
        assert_eq!(outputs[1]["call_id"], "call_b");
        assert!(outputs[1]["output"].as_str().unwrap().contains("beta"));
    }

    #[tokio::test]
    async fn the_loop_stops_at_max_turns_and_at_the_budget() {
        let (url, bodies) = spawn_stub(vec![ok(TURN_WITH_CALLS)]).await;
        let (_dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 3, None).await.unwrap();
        assert_eq!(resp.text, "");
        assert_eq!(resp.metadata.stop_reason.as_deref(), Some("max_turns"));
        assert_eq!(bodies.lock().unwrap().len(), 3);

        let (url, bodies) = spawn_stub(vec![ok(TURN_WITH_CALLS)]).await;
        let resp = run(&url, &mut executor, 10, Some(100)).await.unwrap();
        assert_eq!(
            resp.metadata.stop_reason.as_deref(),
            Some("budget_exhausted")
        );
        assert_eq!(resp.metadata.tokens_used, Some(120));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_response_that_is_not_completed_is_an_error() {
        let text = r#"{"type": "message", "content": [{"type": "output_text", "text": "The answer is tru"}]}"#;
        let call = r#"{"type": "function_call", "call_id": "c", "name": "write",
                       "arguments": "{\"file_path\":\"a.t"}"#;
        let cases = [
            (
                format!(r#"{{"status": "incomplete", "incomplete_details": {{"reason": "max_output_tokens"}}, "output": [{text}]}}"#),
                "the response is incomplete (max_output_tokens)",
            ),
            (
                format!(r#"{{"status": "incomplete", "incomplete_details": {{"reason": "content_filter"}}, "output": [{call}]}}"#),
                "the response is incomplete (content_filter)",
            ),
            (
                r#"{"status": "failed", "error": {"code": "server_error", "message": "boom"}, "output": []}"#.to_string(),
                "the response is failed (server_error: boom)",
            ),
            (
                r#"{"status": "cancelled", "output": []}"#.to_string(),
                "the response is cancelled (no reason given)",
            ),
            (
                r#"{"status": "queued", "output": []}"#.to_string(),
                "the response is queued",
            ),
        ];
        for (reply, expected) in cases {
            let (url, bodies) = spawn_stub(vec![ok(&reply)]).await;
            let (dir, mut executor) = workspace();
            let err = run(&url, &mut executor, 5, None).await.unwrap_err();
            assert!(err.to_string().contains(expected), "{expected}: {err}");
            // No tool ran and nothing was sent back.
            assert_eq!(bodies.lock().unwrap().len(), 1);
            assert!(!dir.path().join("a.t").exists());
        }
    }

    #[tokio::test]
    async fn a_call_with_unparsable_arguments_is_answered_not_run() {
        let turn = r#"{"status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "Reading."}]},
            {"type": "function_call", "call_id": "c1", "name": "write",
             "arguments": "{\"file_path\":\"x.txt\",\"content\":"}]}"#;
        let (url, bodies) = spawn_stub(vec![ok(turn), ok(FINAL_MESSAGE)]).await;
        let (dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 5, None).await.unwrap();
        assert_eq!(resp.text, "a.txt says alpha; b.txt says beta.");
        assert!(!dir.path().join("x.txt").exists());

        // The message sent with the call goes back too, before the output.
        let bodies = bodies.lock().unwrap();
        let input = bodies[1]["input"].as_array().unwrap();
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[3]["call_id"], "c1");
        let output = input[3]["output"].as_str().unwrap();
        assert!(
            output.contains("not valid JSON") && output.contains("not run"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn a_final_answer_is_returned_even_past_the_budget() {
        let (url, _) = spawn_stub(vec![ok(FINAL_MESSAGE)]).await;
        let (_dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 5, Some(1)).await.unwrap();
        assert_eq!(resp.text, "a.txt says alpha; b.txt says beta.");
        assert_eq!(resp.metadata.stop_reason, None);
    }

    #[tokio::test]
    async fn the_budget_counts_input_and_output_when_the_total_is_missing() {
        let turn = r#"{"status": "completed",
            "output": [{"type": "function_call", "call_id": "c", "name": "read",
                        "arguments": "{\"file_path\":\"a.txt\"}"}],
            "usage": {"input_tokens": 500, "output_tokens": 50}}"#;
        let (url, bodies) = spawn_stub(vec![ok(turn)]).await;
        let (_dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 10, Some(100)).await.unwrap();
        assert_eq!(
            resp.metadata.stop_reason.as_deref(),
            Some("budget_exhausted")
        );
        assert_eq!(resp.metadata.tokens_used, Some(550));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_arguments_run_the_tool_with_no_arguments() {
        let turn = r#"{"status": "completed", "output": [
            {"type": "function_call", "call_id": "c", "name": "read", "arguments": " "}]}"#;
        let (url, bodies) = spawn_stub(vec![ok(turn), ok(FINAL_MESSAGE)]).await;
        let (_dir, mut executor) = workspace();
        run(&url, &mut executor, 5, None).await.unwrap();
        // The tool ran and answered with its own error about the missing
        // parameter, not the "not valid JSON" text.
        let bodies = bodies.lock().unwrap();
        let output = bodies[1]["input"][2]["output"].as_str().unwrap();
        assert!(output.contains("file_path"), "{output}");
        assert!(!output.contains("not valid JSON"), "{output}");
    }

    #[test]
    fn arguments_are_parsed_with_empty_as_an_empty_object() {
        assert_eq!(call_arguments("").unwrap(), json!({}));
        assert_eq!(call_arguments(" \n").unwrap(), json!({}));
        assert_eq!(call_arguments(r#"{"a":1}"#).unwrap(), json!({"a": 1}));
        assert!(call_arguments(r#"{"a":"#).is_err());
    }

    #[test]
    fn usage_totals_fall_back_to_input_plus_output() {
        let usage = |t, i, o| ResponsesUsage {
            total_tokens: t,
            input_tokens: i,
            output_tokens: o,
        };
        assert_eq!(usage(None, None, None).total(), None);
        assert_eq!(usage(Some(7), Some(1), Some(1)).total(), Some(7));
        assert_eq!(usage(Some(0), Some(5), Some(5)).total(), Some(0));
        assert_eq!(usage(None, Some(5), None).total(), Some(5));
        assert_eq!(usage(None, None, Some(4)).total(), Some(4));
        assert_eq!(usage(None, Some(5), Some(4)).total(), Some(9));
        assert_eq!(usage(None, Some(u64::MAX), Some(1)).total(), Some(u64::MAX));
    }

    #[tokio::test]
    async fn token_counts_saturate_instead_of_wrapping() {
        let turn = format!(
            r#"{{"status": "completed",
                "output": [{{"type": "function_call", "call_id": "c", "name": "read",
                             "arguments": "{{\"file_path\":\"a.txt\"}}"}}],
                "usage": {{"total_tokens": {}}}}}"#,
            u64::MAX
        );
        let (url, _) = spawn_stub(vec![ok(&turn)]).await;
        let (_dir, mut executor) = workspace();
        let resp = run(&url, &mut executor, 3, Some(1_000)).await.unwrap();
        assert_eq!(
            resp.metadata.stop_reason.as_deref(),
            Some("budget_exhausted")
        );
        assert_eq!(resp.metadata.tokens_used, Some(u64::MAX));

        // Two turns of it with no budget: the sum stays at the maximum.
        let (url, _) = spawn_stub(vec![ok(&turn)]).await;
        let resp = run(&url, &mut executor, 2, None).await.unwrap();
        assert_eq!(resp.metadata.stop_reason.as_deref(), Some("max_turns"));
        assert_eq!(resp.metadata.tokens_used, Some(u64::MAX));
    }

    #[tokio::test]
    async fn http_errors_and_unreadable_items_are_errors() {
        let (url, _) = spawn_stub(vec![(400, r#"{"error":{"message":"bad"}}"#.into())]).await;
        let (_dir, mut executor) = workspace();
        let err = run(&url, &mut executor, 5, None).await.unwrap_err();
        assert!(err.to_string().contains("400"), "{err}");

        let broken = r#"{"output": [{"type": "function_call", "name": "read"}]}"#;
        let (url, _) = spawn_stub(vec![ok(broken)]).await;
        let err = run(&url, &mut executor, 5, None).await.unwrap_err();
        assert!(err.to_string().contains("unreadable output item"), "{err}");
    }

    #[test]
    fn message_text_and_refusals_are_read() {
        let output: Vec<Value> = serde_json::from_str(
            r#"[{"type": "message", "content": [
                    {"type": "output_text", "text": "one "},
                    {"type": "refusal", "refusal": "no"},
                    {"type": "output_text", "text": "two"}]},
                {"type": "web_search_call", "id": "ws_1"}]"#,
        )
        .unwrap();
        let turn = read_turn(&output, "openai").unwrap();
        assert_eq!(turn.text, "one two");
        assert!(turn.calls.is_empty());

        let refused: Vec<Value> = serde_json::from_str(
            r#"[{"type": "message", "content": [{"type": "refusal", "refusal": "I can't."}]}]"#,
        )
        .unwrap();
        assert_eq!(read_turn(&refused, "openai").unwrap().text, "I can't.");
    }

    #[test]
    fn an_empty_system_prompt_sends_no_instructions() {
        let input = build_input(&user("hi"));
        let json = serde_json::to_value(request("gpt-6-luna", "", &input, None)).unwrap();
        assert!(json.get("instructions").is_none());
        assert!(json.get("tools").is_none());
        assert_eq!(json["input"], json!([{"role": "user", "content": "hi"}]));
    }

    #[test]
    fn only_a_reasoning_model_gets_reasoning_fields() {
        let input = build_input(&user("hi"));
        let json = serde_json::to_value(request("gpt-4o", "", &input, None)).unwrap();
        assert!(json.get("reasoning").is_none(), "{json}");
        assert!(json.get("include").is_none(), "{json}");
        assert_eq!(json["store"], false);

        let json = serde_json::to_value(request("gpt-6-luna", "", &input, None)).unwrap();
        assert_eq!(json["reasoning"], json!({"effort": "low"}));
        assert_eq!(json["include"], json!(["reasoning.encrypted_content"]));
    }

    async fn single(url: &str) -> Result<Response, KernexError> {
        responses_complete(
            &reqwest::Client::new(),
            url,
            "Bearer k",
            "gpt-6.2-sol",
            "Be brief.",
            &user("hi"),
            "openai",
        )
        .await
    }

    #[tokio::test]
    async fn a_call_without_tools_sends_one_stateless_request() {
        let (url, bodies) = spawn_stub(vec![ok(FINAL_MESSAGE)]).await;
        let resp = single(&url).await.unwrap();
        assert_eq!(resp.text, "a.txt says alpha; b.txt says beta.");
        assert_eq!(resp.metadata.tokens_used, Some(180));
        assert_eq!(
            resp.metadata.model.as_deref(),
            Some("gpt-6.2-sol-2026-11-01")
        );

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        assert_eq!(body["model"], "gpt-6.2-sol");
        assert_eq!(body["instructions"], "Be brief.");
        assert_eq!(body["input"], json!([{"role": "user", "content": "hi"}]));
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert!(body.get("tools").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_call_without_tools_reports_a_cut_off_or_empty_answer() {
        let cut = r#"{"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"},
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "par"}]}]}"#;
        let (url, _) = spawn_stub(vec![ok(cut)]).await;
        let err = single(&url).await.unwrap_err().to_string();
        assert!(err.contains("incomplete (max_output_tokens)"), "{err}");

        let empty = r#"{"status": "completed", "output": [], "usage": {"input_tokens": 4, "output_tokens": 0}}"#;
        let (url, _) = spawn_stub(vec![ok(empty)]).await;
        let resp = single(&url).await.unwrap();
        assert_eq!(resp.text, "No response from openai.");
        assert_eq!(resp.metadata.tokens_used, Some(4));
    }
}
