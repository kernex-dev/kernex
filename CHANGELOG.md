# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
