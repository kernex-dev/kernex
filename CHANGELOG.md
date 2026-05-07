# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-05-07

### BREAKING

This release closes audit item **M5** (per-crate `thiserror` enums replacing the stringified `KernexError::Variant(String)` shape) and re-architects error types so callers can pattern-match on the typed cause via `Error::source()` chain inspection or `Box::downcast_ref::<T>()`.

#### KernexError variant shape changed (kernex-core 0.5.0)

The cross-crate variants `Provider`, `Store`, `Sandbox`, `Pipeline`, and `Skill` now wrap a `Box<dyn std::error::Error + Send + Sync + 'static>` instead of a `String`. Callers that pattern-matched on the inner string must update:

```rust
// before
match err {
    KernexError::Provider(s) if s.contains("timeout") => /* retry */,
    _ => /* other */,
}

// after
match err {
    KernexError::Provider(boxed) => {
        if let Some(p) = boxed.downcast_ref::<kernex_providers::ProviderError>() {
            match p {
                ProviderError::Http { source, .. } if source.is_timeout() => /* retry */,
                ProviderError::Serde { .. } => /* parse error */,
                _ => /* other */,
            }
        }
    }
    _ => /* other */,
}
```

`Config` and `Guardrail` variants are unchanged (still `String`); they carry no foreign source to preserve.

The new `KernexError` exposes typed constructors (`KernexError::provider(e)`, `::store(e)`, `::sandbox(e)`, `::pipeline(e)`, `::skill(e)`) that accept any `E: Error + Send + Sync + 'static` and box internally.

#### New per-crate error types

- **`kernex-memory`** — `MemoryError` with `Sqlite { context, source }`, `Io { context, source }`, `Serde { context, source }`, `Logic(String)` variants. `From<MemoryError>` for `KernexError` boxes into `KernexError::Store`.
- **`kernex-providers`** — `ProviderError` with `Http { context, source }`, `Serde { context, source }`, `Io { context, source }`, `Config(String)`, `Logic(String)` variants. `Config` hoists to `KernexError::Config`; everything else boxes into `KernexError::Provider`.
- **`kernex-pipelines`** — `PipelineError` with `Toml { context, source }`, `Io { context, source }`, `Logic(String)` variants. Boxes into `KernexError::Pipeline`.
- **`kernex-skills`** — `SkillError` with `Io { context, source }`, `Logic(String)` variants. Boxes into `KernexError::Skill`.

Each per-crate enum implements `std::error::Error` via `thiserror`; `Send + Sync + 'static` bounds are satisfied automatically. The `#[source]` attribute on struct variants preserves the cause chain so `error.source()` walks to the underlying `sqlx::Error`/`reqwest::Error`/etc.

#### Why this design (citations)

- **Rust API Guidelines C-GOOD-ERR** (https://rust-lang.github.io/api-guidelines/interoperability.html): "Error types should always implement the std::error::Error trait... error types should implement the Send and Sync traits."
- **BurntSushi, *Rust Error Handling*** (https://burntsushi.net/rust-error-handling/): libraries should "define your own error type and implement the std::error::Error trait" with structured enum variants, not opaque strings.
- **`tower::Service` precedent** (https://docs.rs/tower/latest/tower/trait.Service.html): for traits used via `dyn Trait` where multiple impls have different error types, the canonical pattern is per-impl typed errors with `Box<dyn Error + Send + Sync>` boxing at the dispatch boundary.

The aggregator stays in `kernex-core` (rather than moving to `kernex-runtime`) because the boxed-trait-object pattern resolves the dependency-cycle concern without restructuring: each per-crate crate provides its own `From<TheirError> for KernexError` impl, so `kernex-core` never has to depend on `kernex-providers`, `kernex-memory`, etc.

### Migration notes for downstream crates

- `kernex-agent` consumes `kernex-runtime` via `anyhow::Result` end-to-end; **no source changes required** (the agent does not pattern-match on `KernexError` variants).
- Direct downstream users that match on `KernexError::Provider(String)` etc. must switch to the downcast pattern shown above.
- The stringified message is still recoverable via `format!("{err}")` for logging — the `#[error(transparent)]` Display delegates through the boxed source, so log output is unchanged.

## [0.4.2] - 2026-05-07

### Security

- **kernex-providers**: bound streaming response buffers (1 MiB Anthropic SSE; 8 MiB MCP `LinesCodec`) so a hostile or runaway server cannot exhaust memory; stop cloning the API key into a local `String` before each request and pass the `SecretString` reference straight into the `Authorization` / `x-api-key` header.
- **kernex-providers / kernex-skills**: drop dynamic-linker env vars (`LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`) from skill-supplied environment maps before spawning MCP servers and toolboxes; closes the sandbox-escape vector flagged as N13.
- **kernex-memory**: pre-create the SQLite file at mode `0o600` *before* sqlx connects, closing the TOCTOU window where `sqlx::sqlite::SqliteConnectOptions::create_if_missing(true)` briefly created a world-readable file (N6).
- **kernex-core**: nested-arg-aware permission checks — `flatten_strings` now recurses into JSON arrays/objects so payloads cannot smuggle dangerous arguments through nesting; `MAX_ARGS_LEN = 64 KiB` cap on the flattened representation.
- **kernex-providers**: `validate_gemini_model_id` enforces `[A-Za-z0-9._-]{1,128}`; `claude_code/command.rs::looks_like_cli_flag` filters values starting with `-` from `model`, `session_id`, and `agent_name` so context-poisoned strings cannot inject CLI flags into the spawned `claude` invocation. `parse_retry_after` honors the `Retry-After` header (clamped to 30 s, max with exponential backoff).
- **kernex-providers**: MCP `protocolVersion` pinned to `"2025-03-26"` — a published spec date — so MCP servers cannot probe for unreleased protocol behaviors via wildcard or future-dated handshakes (N25).

### Added

- **kernex-core**: MSRV polyfill `utf8::floor_char_boundary()` so the workspace continues to support Rust 1.74 even though `str::floor_char_boundary` only stabilizes in 1.83. Used at four sanitization sites.
- **kernex-memory**: prompt-cache token breakdown persisted (migration `016_cache_token_breakdown.sql` adds `cache_read_input_tokens`, `cache_creation_input_tokens`, `prompt_tokens`, and `completion_tokens` to `token_usage`); `Store::record_usage_full(UsageBreakdown)` and `Store::get_total_usage()` round-trip the new columns.
- **kernex-providers**: `cache_read_input_tokens` and `cache_creation_input_tokens` exposed on `AnthropicUsage` and surfaced in `CompletionMeta` so callers can attribute cache hits.

### Changed

- **kernex-runtime**: `warn_if_data_dir_unusual()` emits a `tracing::warn!` when `KERNEX_DATA_DIR` resolves outside `$HOME` / `/tmp` / `/var`, catching the common typo class where data ends up in `/`.
- **kernex-memory**: `Store::consolidator::spawn_cancellable` accepts an `Option<watch::Receiver<bool>>` for cooperative shutdown; `SystemTime::duration_since` no longer silently swallows clock-jump errors via `unwrap_or_default()`.
- **kernex-runtime**: `Runtime::complete*` now constructs a `UsageBreakdown` and routes through `record_usage_full`, replacing the truncating two-column write path.

## [0.4.1] - 2026-05-07

### Added

- **kernex-runtime**: `RuntimeBuilder::auto_compact(bool)` opt-in. When enabled, the runtime reuses the active provider as a `Summarizer` (one extra round-trip per overflow event, not per turn) and prepends `[Earlier conversation summary]` to the system prompt instead of silently dropping the oldest messages. Default off for v0.4.x backward compatibility.
- **kernex-core**: `ContextNeeds` now derives `Debug, Clone` so callers can override individual fields without rebuilding the struct.
- **kernex (umbrella crate)**: re-exports `kernex_runtime::*` so `cargo add kernex` works.

### Changed

- **kernex-memory**: `Store::build_context` now emits `tracing::warn!` when history overflows `max_context_messages` and the Drop strategy is in effect (no summarizer wired in). One log per overflow event with conversation id and overflow count, pointing operators at `RuntimeBuilder::auto_compact`.

### Security

- **kernex-providers**: bumped `rustls-webpki` to 0.103.13 via lock file (RUSTSEC-2026-0098 / 0099 name-constraint bypasses, RUSTSEC-2026-0104 reachable panic in CRL parsing). Bumped `rand` to 0.8.6 / 0.9.4 (RUSTSEC-2026-0097 unsoundness warning).
- **workspace**: `serde_yml` (RUSTSEC-2025-0068, archived crate, unsound) replaced by `serde_yaml_ng`.
- **kernex-memory**: `memory.db` and parent dir now chmod 0600 / 0700 on Unix.
- **kernex-providers**: web fetch tool rejects loopback / RFC1918 / link-local / CGNAT / multicast / metadata IPs (IPv4 and IPv6); `redirect::Policy::none()` so attacker-controlled redirects can't bypass the check. Grep tool caps regex pattern length and passes `--regex-size-limit 16M` to ripgrep.
- **kernex-pipelines**: agent and topology names validated as path segments (no `..`, no separators, no high-bit characters); canonical-prefix symlink guard.
- **kernex-sandbox**: Seatbelt SBPL injection blocked at every interpolation site; new `SandboxProfile::require_os_enforcement` flag returns `ErrorKind::Unsupported` on hosts lacking Seatbelt/Landlock; Landlock `pre_exec` is now allocator-safe (build_ruleset in parent, child only locks Mutex and calls restrict_self).
- **kernex-providers**: error bodies truncated at 16 KB; HTTPS required for keyed providers.
- **kernex-skills**: `Permissions::allows_command` enforced for MCP servers and toolboxes.
- **CI**: `cargo deny check` runs the full advisories + bans + licenses + sources suite. New `deny.toml` rejects openssl / native-tls (rustls-only policy) and pins the license allow-list.

### Added (continued)

- **kernex-providers**: Groq, Mistral, DeepSeek, Fireworks, and xAI as named provider strings in `ProviderFactory::create()` — each resolves to the OpenAI provider with the correct `base_url` and default model. Provider count: 11 built-in (+ Bedrock feature-gated).
- **kernex-providers**: AWS Bedrock provider with SigV4 request signing — supports Anthropic Claude models on Bedrock; opt-in via the `bedrock` Cargo feature.
- **kernex-runtime**: `Runtime::complete_stream()` and `complete_stream_with_needs()` — surfaces streaming from `StreamingProvider` implementations through the public `Runtime` API, returning a `tokio::sync::mpsc::Receiver<StreamEvent>`.
- **kernex-runtime**: `RuntimeBuilder::from_file(path)` — load a declarative TOML or YAML agent definition; maps `[runtime]` and `[memory]` sections into the builder without Rust code.
- **kernex-runtime**: `opentelemetry` optional Cargo feature — enables `tracing-opentelemetry` and `opentelemetry` crates for export to Jaeger, Honeycomb, Grafana, and any OTel-compatible backend.
- **kernex-core**: `GuardrailRunner` trait with `check_input` / `check_output` lifecycle hooks and `GuardrailAction::Allow`, `Block(String)`, `Sanitize(String)` outcomes; injectable via `RuntimeBuilder::with_guardrails()`.
- **kernex-memory**: `PhaseCheckpoint` — pipeline run checkpointing to SQLite; `Store::upsert_phase_checkpoint()` and `Store::get_run_checkpoints()` enable crash-safe phase state tracking (pending, in_progress, completed, failed).
- **kernex-core**: `MemoryConfig::max_connections` — configurable SQLite connection pool size (default: 4).
- **kernex-providers**: `timeout_secs` field on all five HTTP provider TOML configs (`AnthropicConfig`, `OpenAiConfig`, `OllamaConfig`, `OpenRouterConfig`, `GeminiConfig`) and `ProviderConfig::timeout_secs` in `ProviderFactory` (default: 120 s); all HTTP providers expose a `with_timeout(secs) -> Self` builder.
- **kernex-pipelines**: `Phase::parallel_group` — optional named group field; consecutive phases sharing a name are collected by `LoadedTopology::phase_groups()` into `PhaseGroup` structs for concurrent execution. Single-phase groups remain sequential.
- **kernex-runtime**: `full_stack` example — end-to-end demo using `MockProvider`, memory, skills, and a 2-phase pipeline with corrective loop; no API key required.

### Changed

- **docs**: Landlock module doc updated with full ABI version table (V1/5.13 through V5/6.12), WSL1 and container edge cases, and partial-enforcement warning behavior.
- **kernex-sandbox** (`lib.rs`): Linux Landlock description clarified — 5.13 is the minimum for any OS-level enforcement; 6.12 gives full ABI::V5 coverage; older kernels apply best-effort protection.

## [0.4.0] - 2026-03-09

### Added

- **kernex-runtime**: `Runtime::run()` with `RunConfig` and `RunOutcome` — wraps provider completions with max-turns enforcement and fires the `on_stop` hook after each turn
- **kernex-core**: `HookRunner` trait with `pre_tool` / `post_tool` / `on_stop` lifecycle methods and `HookOutcome::Allow` / `Blocked(String)` variants
- **kernex-core**: `NoopHookRunner` default implementation (no-op, zero overhead)
- **kernex-providers**: Prompt caching support for Anthropic — split system prompt at `CACHE_BOUNDARY` marker into a cacheable stable prefix and a dynamic suffix; adds `anthropic-beta: prompt-caching-2024-07-31` header automatically when caching is active
- **kernex-skills**: 12 builtin agent skills in `examples/skills/builtin/` — Tier 1: `frontend-developer`, `backend-architect`, `security-engineer`, `devops-automator`, `reality-checker`, `api-tester`, `performance-benchmarker`; Tier 2: `senior-developer`, `ai-engineer`, `accessibility-auditor`, `agents-orchestrator`, `project-manager`
- **docs**: `docs/kernex-agent.md` — implementation spec for the `kx` CLI binary with provider resolution, runtime wiring, hook runner scaffold, and KAIROS scheduler reference

## [0.3.2] - 2026-03-07

### Fixed

- **kernex-memory**: Added missing `macros` feature to tokio dependency (fixes `tokio::join!`)
- **kernex-providers**: Added missing `macros` feature to tokio dependency (fixes `tokio::select!`)

## [0.3.1] - 2026-03-07

### Added

- **docs**: CONTRIBUTING.md with development setup and PR process
- **docs**: ARCHITECTURE.md expanded with custom Provider guide and kernex-agent reference

### Changed

- **crates.io**: Added documentation URL, keywords, and categories for discoverability
- **clippy**: Enforced `deny(clippy::unwrap_used, clippy::expect_used)` across all crates

## [0.3.0] - 2026-03-07

### Added

- **kernex-providers**: SandboxProfile integration for AI Providers and ProviderFactory
- **kernex-runtime**: `Runtime::complete()` API for simplified completions
- **fuzz**: cargo-fuzz support for `truncate_output`

### Changed

- Anonymized codebase by removing original project name references

## [0.2.0] - 2026-03-06

### Added

- **kernex-core**: `env` field on `McpServer` for passing environment variables to MCP server processes
- **kernex-skills**: `mcp.json` support — load MCP servers from optional JSON file in skill directories, merged with frontmatter (JSON takes precedence on name collision)
- **kernex-skills**: `AGENTS.md` support as modern alternative to `ROLE.md` for project instructions
- **kernex-providers**: Environment variables propagated to MCP subprocess spawn and Claude Code settings
- 4 runnable examples: `simple_chat`, `memory_agent`, `skill_loader`, `pipeline_loader`
- 7 reference skills: filesystem, git, playwright, github, postgres, sqlite, brave-search
- Code-review pipeline topology example with 4 agents
- 298 tests across all crates (+12 from v0.1.0)

## [0.1.0] - 2026-03-06

### Added

- **kernex-core**: Shared types (`Request`, `Response`, `Context`), traits (`Provider`, `Store`), config loading, input sanitization
- **kernex-sandbox**: OS-level sandboxing with Seatbelt (macOS) and Landlock (Linux), code-level path protection
- **kernex-providers**: 6 AI providers (Claude Code CLI, Anthropic, OpenAI, Ollama, OpenRouter, Gemini), tool executor with sandbox enforcement, MCP client over stdio
- **kernex-memory**: SQLite-backed persistent memory with 13 migrations, FTS5 full-text search, conversation lifecycle, user facts, scheduled tasks with dedup, reward-based learning (outcomes/lessons), project-scoped sessions
- **kernex-skills**: Skill loader compatible with Skills.sh standard (`SKILL.md` + TOML/YAML frontmatter), project loader (`ROLE.md`), trigger matching, MCP server activation, flat-to-directory migration
- **kernex-pipelines**: TOML-defined topology format for multi-agent pipelines, phase types (standard, parse-brief, corrective-loop, parse-summary), model tier selection, pre/post validation, agent .md loading
- **kernex-runtime**: Facade crate with `RuntimeBuilder` for composing all subsystems
- Dual license: Apache-2.0 OR MIT
- 286 tests across all crates
