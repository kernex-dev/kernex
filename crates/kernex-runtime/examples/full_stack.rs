//! Full-stack Kernex demo: MockProvider + memory + skills + 2-phase pipeline.
//!
//! Run: `cargo run --example full_stack`
//!
//! Demonstrates the complete runtime stack without any API key:
//!
//!   RuntimeBuilder -> facts/lessons/outcomes in memory -> skill trigger
//!   matching -> Phase 1 (Analyst) -> Phase 2 (Synthesizer) -> memory stats
//!
//! The `examples/` directory is used as the data dir, so the builtin skills
//! under `examples/skills/builtin/` are loaded automatically.
//!
//! Requires the `sqlite-store` feature (enabled by default).

use async_trait::async_trait;
use kernex_core::{
    context::Context,
    error::KernexError,
    message::{CompletionMeta, Request, Response},
    traits::Provider,
};
use kernex_runtime::RuntimeBuilder;
use kernex_skills::{build_skill_prompt, match_skill_triggers};
use tracing_subscriber::EnvFilter;

// --- MockProvider ---

/// A scripted provider that returns a fixed response. No API key required.
///
/// Drop-in replacement for any real provider during development and testing.
struct MockProvider {
    label: &'static str,
    response: &'static str,
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        self.label
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn complete(&self, context: &Context) -> Result<Response, KernexError> {
        tracing::debug!(
            provider = self.label,
            history_messages = context.history.len(),
            "mock completion"
        );
        Ok(Response {
            text: self.response.to_string(),
            metadata: CompletionMeta {
                provider_used: self.label.to_string(),
                tokens_used: Some(42),
                processing_time_ms: 0,
                model: Some("mock-1.0".to_string()),
                session_id: None,
            },
        })
    }
}

// --- Demo ---

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("=== Kernex Full-Stack Demo ===\n");
    println!("Stack: RuntimeBuilder -> memory -> skills -> 2-phase pipeline\n");

    // Point data_dir at the workspace examples/ directory so the builtin skills
    // under examples/skills/builtin/ are loaded without any setup step.
    let examples_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");

    // [1/5] Build the runtime.
    //
    // .db_path(":memory:") keeps the demo self-contained — no files written.
    // In production, omit db_path and let it default to {data_dir}/memory.db.
    let system_prompt = "You are a precise Rust code review assistant. \
        Always cite line numbers. Flag .unwrap() as critical.";

    let runtime = RuntimeBuilder::new()
        .data_dir(examples_dir)
        .db_path(":memory:")
        .system_prompt(system_prompt)
        .channel("demo")
        .project("full-stack-demo")
        .build()
        .await?;

    println!("[1/5] Runtime initialized");
    println!("  Skills loaded: {}", runtime.skills.len());
    println!(
        "  System prompt: \"{}...\"",
        &system_prompt[..system_prompt.len().min(55)]
    );
    println!();

    // [2/5] Pre-populate memory.
    //
    // Facts persist across sessions (user profile).
    // Lessons encode reward-based learning (what worked, what didn't).
    // Outcomes are raw reward signals from user feedback.
    let user = "demo-engineer";
    let project = "full-stack-demo";

    runtime.store.store_fact(user, "language", "Rust").await?;
    runtime
        .store
        .store_fact(user, "style", "functional, no unwrap()")
        .await?;
    runtime
        .store
        .store_fact(user, "role", "Backend Engineer")
        .await?;

    runtime
        .store
        .store_lesson(
            user,
            "code_review",
            "Flag all .unwrap() and .expect() calls as critical — they panic in production.",
            project,
        )
        .await?;
    runtime
        .store
        .store_lesson(
            user,
            "communication",
            "Number action items and include before/after code snippets.",
            project,
        )
        .await?;

    runtime
        .store
        .store_outcome(
            user,
            "review_quality",
            5,
            "Previous review caught a critical path traversal bug.",
            "user",
            project,
        )
        .await?;

    let facts = runtime.store.get_all_facts().await?;
    let lessons = runtime.store.get_lessons(user, None).await?;

    println!("[2/5] Memory pre-populated");
    println!("  Facts ({}):", facts.len());
    for (k, v) in &facts {
        println!("    {k} = {v}");
    }
    println!("  Lessons ({}):", lessons.len());
    for (domain, rule, _project) in &lessons {
        let preview = &rule[..rule.len().min(65)];
        println!("    [{domain}] {preview}...");
    }
    println!("  Outcome: review_quality +5");
    println!();

    // [3/5] Skill trigger matching.
    //
    // The runtime matches skill triggers against each incoming message and
    // injects matched MCP servers + toolboxes into the provider context.
    // Here we call the same functions directly to show what would activate.
    let phase1_input = "analyze this Rust code: \
        fn load(path: &str) { let f = File::open(path).unwrap(); }";

    let matched = match_skill_triggers(&runtime.skills, phase1_input);
    let skill_ctx = build_skill_prompt(&runtime.skills);

    println!("[3/5] Skill trigger matching");
    println!(
        "  Message: \"{}...\"",
        &phase1_input[..phase1_input.len().min(50)]
    );
    println!("  Skills matched: {}", matched.len());
    for srv in &matched {
        println!(
            "    -> {} ({} {})",
            srv.name,
            srv.command,
            srv.args.join(" ")
        );
    }
    if !skill_ctx.prompt.is_empty() {
        let line_count = skill_ctx.prompt.lines().count();
        println!("  System prompt enriched: {line_count} lines injected");
    } else if runtime.skills.is_empty() {
        println!("  Hint: copy examples/skills/ to ~/.kernex/skills/ to activate skills");
    }
    println!();

    // [4/5] Phase 1 — Analyst.
    //
    // runtime.complete() builds a context from memory (history, facts, lessons,
    // outcomes), enriches it with matched skills, calls the provider, and
    // persists the exchange — all in one call.
    let analyst = MockProvider {
        label: "mock-analyst",
        response: "ANALYSIS REPORT\n\
            1. CRITICAL [line 1]: `.unwrap()` on File::open panics on missing \
               or unreadable files.\n\
            2. CRITICAL [line 1]: function returns () — callers cannot handle \
               the error.\n\
            3. WARNING  [line 1]: `path` is not validated — path traversal risk.\n\
            Summary: 2 critical, 1 warning. Not production-ready.",
    };

    let req1 = Request::text(user, phase1_input);

    println!("[4/5] Phase 1 — Analyst ({})", analyst.label);
    println!(
        "  Input: \"{}...\"",
        &phase1_input[..phase1_input.len().min(55)]
    );

    let resp1 = runtime.complete(&analyst, &req1).await?;

    println!("  Response:");
    for line in resp1.text.lines() {
        println!("    {line}");
    }
    println!();

    // [5/5] Phase 2 — Synthesizer.
    //
    // Phase 1 output is forwarded as Phase 2 input. The runtime context now
    // includes the Phase 1 exchange from memory, giving the Synthesizer
    // implicit awareness of what the Analyst produced.
    let phase2_input = format!(
        "Create a numbered fix plan with before/after code snippets based \
         on this analysis:\n\n{}",
        resp1.text
    );

    let synthesizer = MockProvider {
        label: "mock-synthesizer",
        response: "FIX PLAN\n\
            1. Return Result to propagate errors.\n\
               Before: fn load(path: &str) { let f = File::open(path).unwrap(); }\n\
               After:  fn load(path: &str) -> std::io::Result<File> { \
               File::open(path) }\n\
            2. Validate path before opening (reject `..` and absolute paths).\n\
            3. Add #[cfg(test)] covering missing-file and invalid-path cases.\n\
            Confidence: HIGH — all issues are mechanical, no design changes needed.",
    };

    let req2 = Request::text(user, &phase2_input);

    println!("[5/5] Phase 2 — Synthesizer ({})", synthesizer.label);
    println!("  Input: Phase 1 analysis ({} chars)", phase2_input.len());

    let resp2 = runtime.complete(&synthesizer, &req2).await?;

    println!("  Response:");
    for line in resp2.text.lines() {
        println!("    {line}");
    }
    println!();

    // Final memory state.
    let (conv_count, msg_count, fact_count) = runtime.store.get_memory_stats(user).await?;
    let total_tokens =
        resp1.metadata.tokens_used.unwrap_or(0) + resp2.metadata.tokens_used.unwrap_or(0);

    println!("=== Pipeline complete ===");
    println!("  Conversations in memory: {conv_count}");
    println!("  Messages stored:         {msg_count}  (2 user + 2 assistant)");
    println!("  Facts stored:            {fact_count}");
    println!("  Tokens (mock):           {total_tokens}");
    println!();
    println!("Replace MockProvider with AnthropicProvider, OllamaProvider, or any other");
    println!("Provider implementation to run this pipeline against a live model.");

    Ok(())
}
