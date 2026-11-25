# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.0](https://github.com/kernex-dev/kernex/compare/v0.11.0...v0.12.0) - 2026-11-25

### Added

- *(providers)* [**breaking**] start MCP servers outside the workspace
- *(providers)* [**breaking**] start MCP servers inside the sandbox

### Fixed

- *(providers)* refuse relative MCP commands and undeclared servers for CLI providers

### Other

- describe the 0.12.0 sandbox, tools and memory as they now work
- declare Rust 1.86 as the minimum and check it in CI

## [0.9.0](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.9.0) - 2026-06-12

### Added

- *(skills)* enforce declared skill permissions; harden MCP command validation ([#57](https://github.com/kernex-dev/kernex/pull/57))
- *(sandbox)* deny subprocess network egress by default with per-tool opt-in ([#55](https://github.com/kernex-dev/kernex/pull/55))

## [0.6.2](https://github.com/kernex-dev/kernex/compare/v0.6.1...v0.6.2) - 2026-05-11

### Other

- swap toml for basic-toml to drop toml_edit from the workspace

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
