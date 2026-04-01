# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
