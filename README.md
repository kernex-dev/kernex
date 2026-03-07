<h1 align="center">
  <br>
  <img src="https://avatars.githubusercontent.com/u/214714388?s=200&v=4" alt="Kernex" width="120">
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
  <a href="#examples">Examples</a> &bull;
  <a href="#skills">Skills</a> &bull;
  <a href="#providers">Providers</a> &bull;
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
- **Dynamic instantiation** — instantiate robust AI Providers completely dynamically from configuration maps using `ProviderFactory`
- **Typed tool schemas** — Auto-generated JSON Schema for tool parameters via `schemars`
- **MCP client** — stdio-based Model Context Protocol for external tool integration
- **Persistent memory** — SQLite-backed conversations, facts, reward-based learning, scheduled tasks
- **Skills.sh compatible** — load skills from `SKILL.md` files with TOML/YAML frontmatter
- **Multi-agent pipelines** — TOML-defined topologies with corrective loops and file-mediated handoffs
- **Trait-based composition** — implement `Provider` or `Store` to plug in your own backends
- **Secure by default** — All API keys are protected in memory with `secrecy::SecretString`

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
kernex-runtime = "0.3"
kernex-core = "0.3"
kernex-providers = "0.3"
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

Kernex supports [Skills.sh](https://skills.sh) compatible skills. 9 ready-to-use skills included:

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
| **webhook** | CLI | Send HTTP webhooks to external services |

See [examples/skills/](examples/skills/) for documentation and templates.

### Creating Custom Skills

```bash
# Copy the template
cp -r examples/skills/_template ~/.kernex/skills/my-skill

# Edit SKILL.md with your triggers and MCP config
# See examples/skills/README.md for full guide
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
