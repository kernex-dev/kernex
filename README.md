<h1 align="center">
  <br>
  <img src="favicon-kernex.png" alt="Kernex" width="120">
  <br>
  Kernex
  <br>
</h1>

<h4 align="center">The Rust runtime for AI agents.</h4>

<p align="center">
  <a href="https://github.com/kernex-dev/kernex/actions"><img src="https://img.shields.io/github/actions/workflow/status/kernex-dev/kernex/ci.yml?branch=main&style=flat-square" alt="CI"></a>
  <a href="https://crates.io/crates/kernex-runtime"><img src="https://img.shields.io/crates/v/kernex-runtime?style=flat-square" alt="crates.io"></a>
  <a href="https://docs.rs/kernex-runtime"><img src="https://img.shields.io/docsrs/kernex-runtime?style=flat-square" alt="docs.rs"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue?style=flat-square" alt="License"></a>
  <a href="https://github.com/kernex-dev/kernex"><img src="https://img.shields.io/github/stars/kernex-dev/kernex?style=flat-square" alt="Stars"></a>
</p>

<p align="center">
  <a href="#prerequisites">Prerequisites</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#runtime-api">Runtime API</a> &bull;
  <a href="#hooks">Hooks</a> &bull;
  <a href="#providers">Providers</a> &bull;
  <a href="#skills">Skills</a> &bull;
  <a href="#contributing">Contributing</a>
</p>

---

**Kernex** is a composable Rust framework for building AI agent systems. It provides sandboxed execution, multi-provider AI backends, persistent memory with reward-based learning, skill loading, and topology-driven multi-agent pipelines — all as independent, embeddable crates.

## Prerequisites

- **Rust 1.74+** — Install from [rustup.rs](https://rustup.rs)
- **Cargo** — Comes with Rust

For running examples:
- **Ollama** (optional) — For local AI without API keys. [Install](https://ollama.com)
- **Node.js 18+** (optional) — For MCP-based skills. [Install](https://nodejs.org)

## Features

- **Sandbox-first execution** — OS-level protection via Seatbelt (macOS) and Landlock (Linux) combined with highly configurable `SandboxProfile` allow/deny lists
- **6 AI providers** — Claude Code CLI, Anthropic, OpenAI, Ollama, OpenRouter, Gemini
- **OpenAI-compatible base URL** — works with LiteLLM, Cerebras, DeepSeek, Hugging Face, and any compatible endpoint
- **Dynamic instantiation** — instantiate any provider from a config map at runtime via `ProviderFactory`
- **Agentic run loop** — `Runtime::run()` with configurable turn limits; providers handle tool dispatch internally
- **Hook system** — intercept every tool call with `HookRunner`: allow, block, audit, or rate-limit before execution
- **Typed tool schemas** — Auto-generated JSON Schema for tool parameters via `schemars`
- **MCP client** — stdio-based Model Context Protocol for external tool integration
- **Persistent memory** — SQLite-backed conversations, facts, reward-based learning, scheduled tasks
- **Skills.sh compatible** — load skills from `SKILL.md` files with TOML/YAML frontmatter; 12 builtin agent personas included
- **Multi-agent pipelines** — TOML-defined topologies with corrective loops and file-mediated handoffs
- **Trait-based composition** — implement `Provider` or `Store` to plug in your own backends
- **Secure by default** — API keys protected in memory via `secrecy::SecretString`; prompt caching support for Anthropic

## Architecture

Kernex is a Cargo workspace with 7 composable crates:

```mermaid
graph TD
    classDef facade fill:#2B6CB0,stroke:#2C5282,stroke-width:2px,color:#fff
    classDef core fill:#4A5568,stroke:#2D3748,stroke-width:2px,color:#fff
    classDef impl fill:#319795,stroke:#285E61,stroke-width:2px,color:#fff

    R[kernex-runtime]:::facade
    C[kernex-core]:::core
    S[kernex-sandbox]:::impl
    P[kernex-providers]:::impl
    M[kernex-memory]:::impl
    K[kernex-skills]:::impl
    PL[kernex-pipelines]:::impl

    R --> C
    R --> S
    R --> P
    R --> M
    R --> K
    R --> PL

    P --> C
    M --> C
    K --> C
    PL --> C
    S -.o|OS Protection| P
```


| Crate | crates.io | Description |
|-------|-----------|-------------|
| [`kernex-core`](crates/kernex-core) | [![](https://img.shields.io/crates/v/kernex-core?style=flat-square)](https://crates.io/crates/kernex-core) | Shared types, traits, config, sanitization |
| [`kernex-sandbox`](crates/kernex-sandbox) | [![](https://img.shields.io/crates/v/kernex-sandbox?style=flat-square)](https://crates.io/crates/kernex-sandbox) | OS-level sandbox (Seatbelt + Landlock) |
| [`kernex-providers`](crates/kernex-providers) | [![](https://img.shields.io/crates/v/kernex-providers?style=flat-square)](https://crates.io/crates/kernex-providers) | 6 AI providers, tool executor, MCP client |
| [`kernex-memory`](crates/kernex-memory) | [![](https://img.shields.io/crates/v/kernex-memory?style=flat-square)](https://crates.io/crates/kernex-memory) | SQLite memory, FTS5 search, reward learning |
| [`kernex-skills`](crates/kernex-skills) | [![](https://img.shields.io/crates/v/kernex-skills?style=flat-square)](https://crates.io/crates/kernex-skills) | Skill/project loader, trigger matching |
| [`kernex-pipelines`](crates/kernex-pipelines) | [![](https://img.shields.io/crates/v/kernex-pipelines?style=flat-square)](https://crates.io/crates/kernex-pipelines) | TOML topology, multi-agent orchestration |
| [`kernex-runtime`](crates/kernex-runtime) | [![](https://img.shields.io/crates/v/kernex-runtime?style=flat-square)](https://crates.io/crates/kernex-runtime) | Facade crate with `RuntimeBuilder` |

## Quick Start

Add Kernex to your project:

```toml
[dependencies]
kernex-runtime = "0.4"
kernex-core = "0.4"
kernex-providers = "0.4"
tokio = { version = "1", features = ["full"] }
```

Send a message and get a response with persistent memory:

```rust
use kernex_runtime::RuntimeBuilder;
use kernex_core::traits::Provider;
use kernex_core::message::Request;
use kernex_providers::factory::ProviderFactory;
use kernex_providers::ProviderConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Elegant, environment-based construction via `from_env()` 
    // Uses $KERNEX_DATA_DIR, $KERNEX_SYSTEM_PROMPT, and $KERNEX_CHANNEL
    let runtime = RuntimeBuilder::from_env().build().await?;

    let mut config = ProviderConfig::default();
    config.model = Some("llama3.2".to_string());
    config.base_url = Some("http://localhost:11434".to_string());

    let provider = ProviderFactory::create("ollama", Some(serde_json::to_value(config)?))?;


    let request = Request::text("user-1", "What is Rust?");
    let response = runtime.complete(&provider, &request).await?;
    println!("{}", response.text);

    Ok(())
}
```

`runtime.complete()` handles the full pipeline: build context from memory → enrich with skills → send to provider → save exchange.

Use individual crates for fine-grained control:

```rust
use kernex_providers::openai::OpenAiProvider;
use kernex_memory::Store;
use kernex_skills::load_skills;
use kernex_pipelines::load_topology;
```

## Runtime API

`RuntimeBuilder` assembles all subsystems. All options are optional — defaults work out of the box:

```rust
use std::sync::Arc;
use kernex_runtime::RuntimeBuilder;

let runtime = RuntimeBuilder::new()
    .data_dir("~/.my-agent")         // persistent data root, default: ~/.kernex
    .system_prompt("You are...")     // base system prompt prepended every turn
    .channel("cli")                  // channel ID for memory scoping
    .project("my-project")           // project scope for facts and lessons
    .hook_runner(Arc::new(my_hooks)) // lifecycle hook runner (see Hooks section)
    .build()
    .await?;
```

Two completion methods:

| Method | When to use |
|--------|-------------|
| `runtime.complete(&provider, &request)` | Single context-enriched turn. Memory is built, provider runs its internal loop. |
| `runtime.run(&provider, &request, &config)` | Explicit turn-limit control. Sets `max_turns` on the context and fires `on_stop` after completion. |

```rust
use kernex_core::run::{RunConfig, RunOutcome};

let config = RunConfig { max_turns: 20 };

match runtime.run(&provider, &request, &config).await? {
    RunOutcome::EndTurn(response) => println!("{}", response.text),
    RunOutcome::MaxTurns => eprintln!("turn limit reached"),
}
```

## Hooks

Implement `HookRunner` to intercept every tool call across all providers:

```rust
use kernex_core::hooks::{HookRunner, HookOutcome};
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug)]
struct AuditHooks;

#[async_trait]
impl HookRunner for AuditHooks {
    async fn pre_tool(&self, tool_name: &str, _input: &Value) -> HookOutcome {
        tracing::info!("tool: {tool_name}");
        HookOutcome::Allow
        // or: HookOutcome::Blocked("reason".into())  to cancel execution
    }
    async fn post_tool(&self, tool_name: &str, _result: &str, is_error: bool) {
        tracing::debug!("{tool_name} finished, error={is_error}");
    }
    async fn on_stop(&self, _final_text: &str) {}
}

let runtime = RuntimeBuilder::new()
    .hook_runner(Arc::new(AuditHooks))
    .build().await?;
```

`pre_tool` runs before dispatch; returning `Blocked` cancels the tool and returns the reason as a tool error. `post_tool` fires after completion. `on_stop` fires at the end of `Runtime::run()`.

## Providers

Kernex ships with 6 built-in AI providers:

| Provider | Module | API Key Required |
|----------|--------|-----------------|
| Claude Code CLI | `claude_code` | No (uses local CLI) |
| Anthropic | `anthropic` | Yes |
| OpenAI | `openai` | Yes |
| Ollama | `ollama` | No (local) |
| OpenRouter | `openrouter` | Yes |
| Gemini | `gemini` | Yes |

### Prompt caching (Anthropic)

Place `KERNEX_CACHE_BOUNDARY` in your system prompt to split it into a cached stable prefix and a dynamic per-turn suffix. Anthropic caches the stable prefix across turns, reducing token costs on long sessions.

```rust
use kernex_providers::anthropic::CACHE_BOUNDARY;

let system = format!(
    "You are a coding assistant.\n{CACHE_BOUNDARY}\nActive project: {}.",
    project_name
);

let runtime = RuntimeBuilder::new()
    .system_prompt(&system)
    .build().await?;
```

Text before the marker gets `cache_control: ephemeral`. Text after is sent as a plain block each turn. The `anthropic-beta: prompt-caching-2024-07-31` header is added automatically when the boundary is present.

### Using any OpenAI-compatible endpoint

The OpenAI provider accepts a custom `base_url`, making it work with any compatible service:

```rust
use kernex_providers::openai::OpenAiProvider;

// LiteLLM proxy
let provider = OpenAiProvider::from_config(
    "http://localhost:4000/v1".into(),
    "sk-...".into(),
    "gpt-4".into(),
    None,
)?;

// DeepSeek
let provider = OpenAiProvider::from_config(
    "https://api.deepseek.com/v1".into(),
    "sk-...".into(),
    "deepseek-chat".into(),
    None,
)?;

// Cerebras
let provider = OpenAiProvider::from_config(
    "https://api.cerebras.ai/v1".into(),
    "csk-...".into(),
    "llama3.1-70b".into(),
    None,
)?;
```

### Implementing a custom provider

```rust
use kernex_core::traits::Provider;
use kernex_core::context::Context;
use kernex_core::message::Response;

#[async_trait::async_trait]
impl Provider for MyProvider {
    fn name(&self) -> &str { "my-provider" }
    fn requires_api_key(&self) -> bool { true }
    async fn is_available(&self) -> bool { true }

    async fn complete(&self, context: &Context) -> kernex_core::error::Result<Response> {
        // Your implementation here
        todo!()
    }
}
```

## Project Structure

```
~/.kernex/                  # Default data directory
├── config.toml             # Runtime configuration
├── memory.db               # SQLite persistent memory
├── skills/                 # Skill definitions
│   └── my-skill/
│       └── SKILL.md        # TOML/YAML frontmatter + instructions
├── projects/               # Project definitions
│   └── my-project/
│       └── AGENTS.md       # Project instructions + skills (or ROLE.md)
└── topologies/             # Pipeline definitions
    └── my-pipeline/
        ├── TOPOLOGY.toml   # Phase definitions
        └── agents/         # Agent .md files
```

## Examples

| Example | Description | Prerequisites | Run |
|---------|-------------|---------------|-----|
| `simple_chat` | Interactive chat with local LLM | Ollama running | `cargo run --example simple_chat` |
| `memory_agent` | Persistent facts and lessons | None | `cargo run --example memory_agent` |
| `skill_loader` | Load skills and match triggers | None | `cargo run --example skill_loader` |
| `pipeline_loader` | Multi-agent topology demo | None | `cargo run --example pipeline_loader` |

All examples are in [`crates/kernex-runtime/examples/`](crates/kernex-runtime/examples/).

## Skills

Kernex supports [Skills.sh](https://skills.sh) compatible skills. 21 ready-to-use skills included: 9 tool-integration skills and 12 builtin agent personas.

### Tool skills

| Skill | Backend | Description |
|-------|---------|-------------|
| **filesystem** | MCP | Secure file operations |
| **git** | MCP | Repository operations |
| **playwright** | MCP | Browser automation |
| **github** | MCP | GitHub API integration |
| **postgres** | MCP | PostgreSQL read-only access |
| **sqlite** | MCP | SQLite read/write access |
| **brave-search** | MCP | Web search via Brave API |
| **pdf** | CLI | Extract text from PDFs |
| **webhook** | CLI | Send HTTP webhooks |

See [examples/skills/](examples/skills/) for documentation and templates.

### Builtin agent skills

12 agent persona skills ship in `examples/skills/builtin/`. Install all at once:

```bash
cp -r examples/skills/builtin/* ~/.kernex/skills/
```

| Skill | Purpose |
|-------|---------|
| `frontend-developer` | UI/UX, component architecture |
| `backend-architect` | APIs, databases, system design |
| `security-engineer` | Threat modeling, secure code review |
| `devops-automator` | CI/CD, infrastructure, containers |
| `reality-checker` | Assumptions audit, edge case analysis |
| `api-tester` | API contract and integration testing |
| `performance-benchmarker` | Profiling and optimization |
| `senior-developer` | Cross-domain code review |
| `ai-engineer` | ML/AI integration patterns |
| `accessibility-auditor` | WCAG, a11y review |
| `agents-orchestrator` | Multi-agent workflow design |
| `project-manager` | Planning, scope, delivery |

Skills activate automatically when a message matches their `triggers` frontmatter. No configuration needed beyond placing the file.

### Creating custom skills

```bash
# Copy the template
cp -r examples/skills/_template ~/.kernex/skills/my-skill

# Edit SKILL.md with your triggers and MCP config
# See examples/skills/README.md for full guide
```

## Common Errors

### "unknown provider type: xyz"

The provider name must match exactly. Valid values: `openai`, `anthropic`, `ollama`, `gemini`, `openrouter`, `claude-code`.

### "config error: failed to create data dir"

Ensure `~/.kernex/` is writable:

```bash
mkdir -p ~/.kernex && chmod 755 ~/.kernex
```

### "provider error: timeout"

The provider took longer than the configured timeout (default 120s). Check your internet connection or increase the timeout in `ProviderConfig`.

### "Ollama not available"

Ollama server isn't running. Start it in a separate terminal:

```bash
ollama serve
```

### "model not found" (Ollama)

Pull the model first:

```bash
ollama pull llama3.2
```

## Development

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --check
```

## Versioning

This project follows [Semantic Versioning](https://semver.org/). All crates in the workspace share the same version number.

- **MAJOR** — breaking API changes
- **MINOR** — new features, backward compatible
- **PATCH** — bug fixes, backward compatible

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Contributing

Contributions are welcome. Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Ensure all checks pass: `cargo build && cargo clippy -- -D warnings && cargo test && cargo fmt --check`
4. Commit with conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`)
5. Open a Pull Request

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
