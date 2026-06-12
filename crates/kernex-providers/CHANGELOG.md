# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
