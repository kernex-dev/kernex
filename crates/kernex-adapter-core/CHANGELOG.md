# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.4](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.8.4) - 2026-05-19

### Other

- *(opsec)* scrub internal planning identifier from 0.8.3 changelog ([#32](https://github.com/kernex-dev/kernex/pull/32))

## [0.8.3](https://github.com/kernex-dev/kernex/compare/v0.8.2...v0.8.3) - 2026-05-19

### Added

- *(adapter-core)* add Detection::project_root for project-local adapter writes (FU-F-01) ([#30](https://github.com/kernex-dev/kernex/pull/30))

### Added

- `Detection::project_root: Option<PathBuf>` field for adapters that write project-local files (e.g., Codex `<cwd>/AGENTS.md`, Cursor `.cursorrules`). Additive on the `#[non_exhaustive]` struct; `#[serde(default)]` keeps the wire format back-compatible with 0.8.2 callers. Resolves ADR-001 in the Codex adapter design notes.
- `Detection::with_project_root(installed, config_root, project_root, version)` constructor for adapters with project-local writes. `Detection::new(installed, config_root, version)` is retained source-compatibly and sets `project_root: None`.

## [0.8.2](https://github.com/kernex-dev/kernex/compare/v0.8.1...v0.8.2) - 2026-05-19

### Added

- *(adapter-core)* add Detection::new public constructor (FU-E-01) ([#28](https://github.com/kernex-dev/kernex/pull/28))

### Added

- `Detection::new(installed, config_root, version)` public constructor (FU-E-01). Lets downstream consumers build the value without routing through `serde_json::from_value` while the struct remains `#[non_exhaustive]`. Wire format is unchanged; pinned by the new `detection_new_roundtrips` smoke test.
