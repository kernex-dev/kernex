# CLAUDE.md — Kernex

## Project

Kernex is a Rust runtime engine for AI agents. It provides sandboxed execution, multi-provider AI backends, reward-based learning, and topology-driven multi-agent pipelines as composable crates.

**Organization:** `github.com/kernex-dev`
**Domain:** `kernex.dev`
**Tagline:** *The Rust runtime for AI agents.*

## Origin

Extracted from a prior internal monolithic codebase, keeping the battle-tested core (sandbox, providers, memory, skills, pipelines) and discarding the monolithic personal-agent shell (hardcoded messaging channels, hardcoded gateway, system prompt).

## Git Rules

1. **Identity:** All commits use `kernex-dev <support@kernex.dev>`. Configured in local `.git/config`.
2. **No Co-Author:** Never append `Co-Authored-By` lines to commit messages. Ever.
3. **No auto-commit/push:** Always ask before committing or pushing.
4. **Commit style:** Conventional commits — `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`, `test:`.
5. **Atomic commits:** One logical change per commit.
6. **Branch protection:** Never force push to `main`.

## Architecture

Cargo workspace with composable crates:

| Crate | Purpose |
|-------|---------|
| `kernex-runtime` | Core engine — trait-based Runtime, agent lifecycle, message pipeline |
| `kernex-sandbox` | OS-level protection — Seatbelt (macOS), Landlock (Linux). Publishable standalone |
| `kernex-providers` | AI backends — Claude Code CLI, Anthropic, OpenAI, Ollama, OpenRouter, Gemini + MCP client |
| `kernex-memory` | Pluggable storage — conversations, learning (rewards/lessons), scheduled tasks |
| `kernex-skills` | Skill loader compatible with Skills.sh standard (`SKILL.md` + frontmatter) |
| `kernex-pipelines` | Topology-driven multi-agent pipelines with file-mediated handoffs and corrective loops |

## Design Principles

1. **The best part is the one you can remove.** Less is more — always.
2. **Framework, not application.** Kernex is embeddable. No hardcoded channels, no hardcoded prompts.
3. **Trait-based composition.** Users implement traits (`Provider`, `Channel`, `Store`) to plug in their own backends.
4. **Skills.sh compatible.** Adopt the Vercel standard for skill format. Don't invent another.
5. **Sandbox-first.** OS-level protection is always on. Code-level checks are defense-in-depth.
6. **No unwrap() in production.** Use `?` and proper error types. Never panic.
7. **Tracing, not println.** Use the `tracing` crate for all logging.
8. **Async everywhere.** Tokio runtime, all I/O is async.
9. **No file > 500 lines** (excluding tests).

## Code Standards

## Code Standards

- Rust edition 2021
- Strict clippy enforcement: `#![deny(clippy::unwrap_used, clippy::expect_used)]` applied to all crate `lib.rs` roots (falling back to `#![cfg_attr(test, allow(...))]` for testing), as well as `#![deny(warnings)]`.
- `cargo fmt` before every commit
- Every public type and function gets a doc comment
- Every new feature includes tests
- No `unsafe` unless absolutely necessary (document why)
- Errors: `thiserror` for library crates, `anyhow` only at binary boundaries

## Pre-Commit Gate

| Step | Action |
|------|--------|
| 1 | `cargo build --workspace` |
| 2 | `cargo audit` and `cargo deny check` (Supply Chain CI Gate) |
| 3 | `cargo clippy --workspace -- -D warnings` |
| 4 | `cargo test --workspace` |
| 5 | `cargo fmt --check` |
| 6 | Commit only after 1-5 pass |

## Versioning & Publishing

- All crates share the same version via `workspace.package.version`.
- **Bump rule:** Minor (0.x → 0.y) for new features or breaking changes. Patch (0.x.y) for bug fixes only.
- **Never publish without asking.** Always confirm before `cargo publish`.
- **Publish order** (leaf crates first): `kernex-core` → `kernex-sandbox` → `kernex-memory` → `kernex-pipelines` → `kernex-skills` → `kernex-providers` → `kernex-runtime`.
- **Version bump commit:** Use `chore: bump version to X.Y.Z` as a separate commit after the feature commit.
- **README in crates.io:** Every crate Cargo.toml has `readme = "../../README.md"` pointing to the workspace README.
- After publishing, verify the crate pages on crates.io show the README correctly.

## What We Keep from the Internal Monolith (proven, battle-tested)

- Sandbox dual-layer design (OS + code-level)
- Provider trait + 6 implementations + tool executor + MCP client
- SQLite memory with migrations, reward-based learning (REWARD/LESSON)
- Skill loader with trigger matching and MCP activation
- Multi-agent pipeline with topologies, corrective loops, file-mediated handoffs
- Prompt sanitization
- Error handling patterns (LegacyError -> KernexError)

## What We Discard from the Internal Monolith

- Telegram/WhatsApp channel implementations (users bring their own)
- Hardcoded gateway pipeline (replaced by composable Runtime)
- System prompt (SYSTEM_PROMPT.md) — configurable, not bundled
- Marker string protocol — replaced by typed API
- i18n module — out of scope for a runtime engine
- CLI wizard (init.rs) — out of scope
- Commands module — out of scope

## What We Fix from the Internal Monolith (audit findings)

- HTTP retry with backoff for all providers (was missing)
- `with_config_path()` wired up (was dead code)
- Dynamic model routing Sonnet/Opus (was dead code)
- Integration tests for auth, pipeline, background loops
- Gateway struct decomposed (was 16-field god object)
- `std::fs` replaced with `tokio::fs` in async context
- WhatsApp MIME extension sanitized (path traversal)
- Google refresh token cleanup (was leaking)
