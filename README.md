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
  <a href="#features">Features</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#providers">Providers</a> &bull;
  <a href="#contributing">Contributing</a> &bull;
  <a href="#license">License</a>
</p>

---

**Kernex** is a composable Rust framework for building AI agent systems. It provides sandboxed execution, multi-provider AI backends, persistent memory with reward-based learning, skill loading, and topology-driven multi-agent pipelines — all as independent, embeddable crates.

## Features

- **Sandbox-first execution** — OS-level protection via Seatbelt (macOS) and Landlock (Linux)
- **6 AI providers** — Claude Code CLI, Anthropic, OpenAI, Ollama, OpenRouter, Gemini
- **OpenAI-compatible base URL** — works with LiteLLM, Cerebras, DeepSeek, Hugging Face, and any compatible endpoint
- **MCP client** — stdio-based Model Context Protocol for external tool integration
- **Persistent memory** — SQLite-backed conversations, facts, reward-based learning, scheduled tasks
- **Skills.sh compatible** — load skills from `SKILL.md` files with TOML/YAML frontmatter
- **Multi-agent pipelines** — TOML-defined topologies with corrective loops and file-mediated handoffs
- **Trait-based composition** — implement `Provider` or `Store` to plug in your own backends

## Architecture

Kernex is a Cargo workspace with 7 composable crates:

```
kernex-runtime          Facade — composes all crates into a RuntimeBuilder
  ├── kernex-core       Shared types, traits (Provider, Store), config, error handling
  ├── kernex-sandbox    OS-level protection (Seatbelt/Landlock)
  ├── kernex-providers  AI backends + tool executor + MCP client
  ├── kernex-memory     SQLite storage, conversations, learning, tasks
  ├── kernex-skills     Skill/project loader, trigger matching, MCP activation
  └── kernex-pipelines  Topology-driven multi-agent pipelines
```

| Crate | Description | Tests |
|-------|-------------|-------|
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
kernex-runtime = "0.1"
tokio = { version = "1", features = ["full"] }
```

Build and initialize a runtime:

```rust
use kernex_runtime::RuntimeBuilder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime = RuntimeBuilder::new()
        .data_dir("~/.my-agent")
        .build()
        .await?;

    println!("Loaded {} skills, {} projects",
        runtime.skills.len(),
        runtime.projects.len());

    Ok(())
}
```

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

Runnable examples in [`crates/kernex-runtime/examples/`](crates/kernex-runtime/examples/):

```bash
# Interactive chat with Ollama (local, no API key)
cargo run --example simple_chat

# Persistent memory: facts, lessons, outcomes
cargo run --example memory_agent

# Load skills and match triggers
cargo run --example skill_loader

# Load and inspect a multi-agent pipeline topology
cargo run --example pipeline_loader
```

Reference skills for common MCP servers in [`examples/skills/`](examples/skills/).

## Development

```bash
# Build all crates
cargo build --workspace

# Run all 290 tests
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
