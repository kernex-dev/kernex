# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.9.0) - 2026-06-12

### Fixed

- *(bedrock)* update default model IDs to current Claude models ([#49](https://github.com/kernex-dev/kernex/pull/49))
- *(anthropic)* rebuild SSE streaming to read usage, stop_reason, and errors ([#47](https://github.com/kernex-dev/kernex/pull/47))
- *(runtime)* surface max_turns as RunOutcome::MaxTurns, stop fabricating answers ([#43](https://github.com/kernex-dev/kernex/pull/43))

## [0.8.0](https://github.com/kernex-dev/kernex/compare/v0.7.0...v0.8.0) - 2026-05-12

### Added

- *(memory)* [**breaking**] typed observations table + save_observation trait surface ([#21](https://github.com/kernex-dev/kernex/pull/21))

## [0.6.1](https://github.com/kernex-dev/kernex/compare/v0.6.0...v0.6.1) - 2026-05-10

### Other

- *(deps)* replace em-dash with period or colon in crate descriptions

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Added

- *(memory)* introduce MemoryStore trait + soft-delete on facts + Runtime::store_handle()
- *(workspace)* split workspace into kernex-adapter-core, kernex-presets, kernex-brain

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- *(deps)* drop unused dependencies flagged by cargo-machete
