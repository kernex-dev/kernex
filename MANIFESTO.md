# The Kernex Manifesto

## Why Another AI Framework?

The existing AI agent landscape is dominated by Python. LangChain, CrewAI, AutoGen, LlamaIndex, Phidata. These tools built the first wave of AI applications and proved the category. But they carry a cost: runtime errors that should have been caught at compile time, GIL bottlenecks in concurrent pipelines, megabyte containers to run trivial agents, and framework magic that makes debugging a guessing game.

Kernex is the answer to a different question: what does an AI agent runtime look like when correctness, performance, and security are first-class requirements rather than afterthoughts?

---

## What Makes Kernex Different

### Rust, Not Python

Every AI framework of consequence runs on Python. Kernex does not.

Rust means:

- Type errors at compile time, not at 3am in production
- No garbage collector pauses mid-conversation
- No GIL contention in concurrent pipelines
- A single compiled binary with no virtualenv, no `requirements.txt`, no container just to run an agent

This is not a performance argument alone. It is a correctness argument. Rust forces explicit error handling, clear ownership, and compile-time concurrency reasoning. AI agents that modify files, invoke shell commands, and call external APIs benefit from that discipline more than most programs.

### OS-Level Sandboxing

Most frameworks operate at the code level: they restrict which functions the agent can call. Kernex goes to the OS.

Seatbelt on macOS and Landlock on Linux wrap every agent subprocess at the kernel level. The AI cannot read files outside the workspace, write to system paths, or open unexpected network connections. Even if a model is deceived into attempting a harmful action, the OS refuses.

Code-level restrictions are defense in depth. OS-level restrictions are the wall.

### Provider-Agnostic by Design

Most SDKs are built by model providers. They work best with that provider's models, and porting to another is friction. Kernex treats providers as interchangeable.

Eleven providers ship today: Claude Code CLI, Anthropic, OpenAI, Ollama, Gemini, OpenRouter, Groq, Mistral, DeepSeek, Fireworks, and xAI. AWS Bedrock is available behind an optional feature flag. The OpenAI provider accepts a custom `base_url`, making it compatible with LiteLLM, Cerebras, Hugging Face, and any OpenAI-compatible endpoint. Swap providers by changing one string. Your agent code does not change.

### Composable Crates, Not a Monolith

Kernex ships as seven independent crates. Use what you need.

Running local agents with Ollama and no persistent memory? Pull `kernex-providers` and `kernex-core`. Building a production pipeline with compliance hooks? Add `kernex-memory` and wire `HookRunner`. Deploying the sandbox in an existing application? `kernex-sandbox` is publishable standalone.

The monolith forces you to accept the whole framework's opinions. Composable crates let you audit and own every dependency.

### Persistent Memory with Reward Learning

Most frameworks treat memory as conversation history. Kernex treats it as a knowledge base that improves over time.

The REWARD and LESSON system lets agents record what worked, what failed, and why. These lessons surface as context in future sessions. An agent that learned a particular API has a 10-second timeout will carry that knowledge forward. This is not fine-tuning. It is structured, session-persistent learning that any developer can inspect and edit directly in SQLite.

### TOML-Defined Pipelines

Multi-agent workflows defined in code are hard to version, review, and hand off. Kernex pipelines are TOML files: diff-able, readable, editable by anyone who can open a text file.

Phases, agents, corrective loops, pre-validation, and parallel execution: all declared in a topology file. Tag consecutive phases with the same `parallel_group` name and they run concurrently. No Python class hierarchies, no decorator chains, no proprietary DSL. A `TOPOLOGY.toml` describes the pipeline the same way a `Cargo.toml` describes a project.

### Skills.sh Compatibility

Kernex does not invent a new skill format. It adopts Skills.sh, the open standard the community has converged on.

Skills are `SKILL.md` files. TOML or YAML frontmatter declares the skill's name, triggers, and permissions. The body is markdown injected into the system prompt. No scripts, no binaries, no executables. Text only.

Every Kernex agent can use community-published skills. Every skill written for Kernex works in any Skills.sh compatible tool.

---

## Kernex vs the Alternatives

| | Kernex | LangChain | CrewAI | AutoGen | OpenAI SDK |
|--|--------|-----------|--------|---------|------------|
| Language | Rust | Python | Python | Python | Python/JS |
| OS sandbox | Yes | No | No | No | No |
| Provider-agnostic | Yes | Partial | Partial | Partial | No |
| Reward learning | Yes | No | No | No | No |
| Composable crates | Yes | No | No | No | No |
| TOML pipelines | Yes | No | No | No | No |
| Skills.sh compatible | Yes | No | No | No | No |
| Single binary deploy | Yes | No | No | No | No |

---

## Who Kernex Is For

### AI Engineers and Framework Builders

You want to build an agent product or internal tool, not orchestrate Python packages. Kernex gives you a trait-based API: implement `Provider` for your custom LLM endpoint, `Store` for your database, `HookRunner` for your compliance layer. If it compiles, the types are correct.

```toml
[dependencies]
kernex-runtime = "0.4"
kernex-core = "0.4"
kernex-providers = "0.4"
```

### Platform and Security Teams

You need agents that run in production without giving the model unrestricted access to your infrastructure. Kernex gives you OS-level sandboxing that enforces boundaries even if the model is compromised, a `HookRunner` for pre-tool auditing and rate limiting, structured audit logging, and provider flexibility to point at a private endpoint with no external API calls required.

### Independent Developers

You want to embed AI reasoning into a Rust application without pulling in a Python runtime or depending on a hosted service. Kernex gives you a single compiled dependency, offline operation via Ollama, and per-project persistent memory that travels with your binary's data directory.

### Teams Building Internal Tools

You have a pipeline: gather data, analyze it, produce a report, validate the output. Kernex pipelines let you define that workflow in a TOML file, assign different agents to different phases, and add corrective loops where a second agent reviews and fixes the first. The topology is a file in your repo, versioned alongside your code.

---

## What Kernex Is Not

Kernex is not a finished product. It is a foundation.

It does not ship a chatbot, a web interface, or a hosted service. It provides the runtime primitives so you can build those things correctly.

It does not force an architecture on you. Use the full `RuntimeBuilder` or compose individual crates. Use six providers or implement your own. Use TOML pipelines or build dynamic ones in code.

The best Kernex application is one where the framework disappears and only your agent remains.

---

## The Principle

> The best part is the one you can remove.

Every crate exists because something needed to exist. Nothing was added for completeness. Nothing was kept for backward compatibility when it stopped earning its place.

Kernex is small by design, correct by construction, and open by default. Build on it.
