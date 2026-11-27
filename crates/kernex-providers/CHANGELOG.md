# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.2](https://github.com/kernex-dev/kernex/compare/v0.14.1...v0.14.2) - 2026-11-27

### Other

- *(providers)* run the Responses loop tests on captured bodies

## [0.14.1](https://github.com/kernex-dev/kernex/compare/v0.14.0...v0.14.1) - 2026-11-27

### Fixed

- *(core)* use llama3.2 as the ollama config default
- *(providers)* keep credentials in a base URL out of errors and logs

## [0.14.0](https://github.com/kernex-dev/kernex/compare/v0.13.1...v0.14.0) - 2026-11-27

### Added

- *(providers)* [**breaking**] send OpenAI's own host through the Responses API
- *(providers)* add a Responses API loop for OpenAI

### Fixed

- *(providers)* keep the compatible providers on Chat Completions

## [0.13.1](https://github.com/kernex-dev/kernex/compare/v0.13.0...v0.13.1) - 2026-11-26

### Changed

- *(providers)* move the openai defaults to gpt-6-luna

## [0.13.0](https://github.com/kernex-dev/kernex/compare/v0.12.0...v0.13.0) - 2026-11-26

### Fixed

- *(providers)* [**breaking**] let the claude-code provider default to no tools

## [0.12.0](https://github.com/kernex-dev/kernex/compare/v0.11.0...v0.12.0) - 2026-11-25

### Added

- [**breaking**] run the agent CLI providers under the tool policy
- *(providers)* [**breaking**] start MCP servers outside the workspace
- *(providers)* [**breaking**] start MCP servers inside the sandbox
- *(providers)* [**breaking**] enforce allowed_tools and match permission rules per argument
- *(providers)* [**breaking**] turn web_fetch off by default and gate it by host
- *(providers)* [**breaking**] confine the file tools to the workspace
- *(sandbox)* [**breaking**] deny by default in the macOS profile
- *(sandbox)* [**breaking**] scope tool writes to the workspace through one path policy

### Fixed

- *(sandbox)* [**breaking**] check a search root and its exclusions against one policy
- *(sandbox)* guard links on the way to a write root, keep root metadata writable
- *(sandbox)* keep write roots from being swapped for a symlink
- *(providers)* [**breaking**] give OpenCode new directories for every call
- *(providers)* refuse relative MCP commands and undeclared servers for CLI providers
- *(providers)* match file rules on the resolved path and filter search results
- *(providers)* close address and pinning gaps in web_fetch
- *(sandbox)* let Linux subprocesses list directories above a protected path
- *(providers)* keep skills readable and bound what the read tool opens

### Other

- describe the 0.12.0 sandbox, tools and memory as they now work
- declare Rust 1.86 as the minimum and check it in CI
- *(providers)* name what the policy must refuse in the MCP probe
- *(providers)* let the sandbox policy pick which MCP writes to probe
- *(providers)* probe MCP server writes the same way from any checkout
- *(providers)* make the process group probes need a real survivor

## [0.11.0](https://github.com/kernex-dev/kernex/compare/v0.10.0...v0.11.0) - 2026-11-21

### Added

- *(providers)* expose ProviderFactory::default_model
- *(providers)* add sandboxed opencode CLI provider

### Fixed

- *(providers)* replace Gemini IDs before the 2026-12-01 shutdown; use listed OpenRouter IDs
- *(providers)* replace retired default and tier model IDs

### Other

- make the README quick start compile and state 12 providers everywhere

## [0.10.0](https://github.com/kernex-dev/kernex/compare/v0.9.0...v0.10.0) - 2026-06-12

### Added

- *(core,providers,runtime)* stop the agentic loop at a token budget ([#59](https://github.com/kernex-dev/kernex/pull/59))

## [0.9.0](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.9.0) - 2026-06-12

### Added

- *(skills)* enforce declared skill permissions; harden MCP command validation ([#57](https://github.com/kernex-dev/kernex/pull/57))
- *(sandbox)* deny subprocess network egress by default with per-tool opt-in ([#55](https://github.com/kernex-dev/kernex/pull/55))
- *(sandbox)* enforce required sandboxing on spawns; warn once when unsandboxed ([#54](https://github.com/kernex-dev/kernex/pull/54))
- *(sandbox)* isolate subprocess environments from provider credentials ([#51](https://github.com/kernex-dev/kernex/pull/51))

### Fixed

- *(providers)* redact api_key in Debug; pin web_fetch to validated addresses ([#52](https://github.com/kernex-dev/kernex/pull/52))
- *(providers)* close four small provider-correctness gaps ([#50](https://github.com/kernex-dev/kernex/pull/50))
- *(bedrock)* update default model IDs to current Claude models ([#49](https://github.com/kernex-dev/kernex/pull/49))
- *(anthropic)* drop the obsolete prompt-caching beta header ([#48](https://github.com/kernex-dev/kernex/pull/48))
- *(anthropic)* rebuild SSE streaming to read usage, stop_reason, and errors ([#47](https://github.com/kernex-dev/kernex/pull/47))
- *(anthropic)* raise default max_tokens and guard against assistant prefill ([#46](https://github.com/kernex-dev/kernex/pull/46))
- *(anthropic)* send adaptive thinking and preserve thinking blocks ([#44](https://github.com/kernex-dev/kernex/pull/44))
- *(runtime)* surface max_turns as RunOutcome::MaxTurns, stop fabricating answers ([#43](https://github.com/kernex-dev/kernex/pull/43))
- *(claude-code)* inject MCP via --mcp-config temp file, not settings.local.json ([#42](https://github.com/kernex-dev/kernex/pull/42))
- *(providers)* refresh default Anthropic models and drop unverified beta header ([#41](https://github.com/kernex-dev/kernex/pull/41))

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- *(deps)* drop unused dependencies flagged by cargo-machete
