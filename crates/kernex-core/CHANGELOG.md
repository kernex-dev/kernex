# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.9.0) - 2026-06-12

### Added

- *(skills)* enforce declared skill permissions; harden MCP command validation ([#57](https://github.com/kernex-dev/kernex/pull/57))
- *(sandbox)* deny subprocess network egress by default with per-tool opt-in ([#55](https://github.com/kernex-dev/kernex/pull/55))

### Fixed

- *(providers)* close four small provider-correctness gaps ([#50](https://github.com/kernex-dev/kernex/pull/50))
- *(anthropic)* rebuild SSE streaming to read usage, stop_reason, and errors ([#47](https://github.com/kernex-dev/kernex/pull/47))
- *(pricing)* correct token rates for current Claude models ([#45](https://github.com/kernex-dev/kernex/pull/45))
- *(anthropic)* send adaptive thinking and preserve thinking blocks ([#44](https://github.com/kernex-dev/kernex/pull/44))
- *(runtime)* surface max_turns as RunOutcome::MaxTurns, stop fabricating answers ([#43](https://github.com/kernex-dev/kernex/pull/43))
- *(providers)* refresh default Anthropic models and drop unverified beta header ([#41](https://github.com/kernex-dev/kernex/pull/41))

## [0.6.2](https://github.com/kernex-dev/kernex/compare/v0.6.1...v0.6.2) - 2026-05-11

### Other

- swap toml for basic-toml to drop toml_edit from the workspace

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
