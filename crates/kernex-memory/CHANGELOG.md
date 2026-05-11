# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.2](https://github.com/kernex-dev/kernex/compare/v0.6.1...v0.6.2) - 2026-05-11

### Performance

- Fast-path migration check in `Store::new`: replace the per-migration `SELECT name FROM _migrations WHERE name = ?` round-trip loop with one `SELECT name FROM _migrations` plus an in-memory `HashSet` lookup. O(N²) network IO becomes O(N); saves ~10 ms per cold open on warm cache.

### Documentation

- Add `openspec/changes/memory-typed-row-shape/` (proposal + tasks) that locks the two-slice plan for typed row shapes on the trait surface. Slice A (this release) ships the perf fast-path. Slice B (deferred to 0.7.0) types `updated_at` as `SystemTime`, pushes `since: Option<SystemTime>` server-side into `search_messages`, and surfaces `MessageRow` / `HistoryRow` from the trait return.

## [0.6.1](https://github.com/kernex-dev/kernex/compare/v0.6.0...v0.6.1) - 2026-05-10

### Other

- *(deps)* replace em-dash with period or colon in crate descriptions

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Added

- *(memory)* introduce MemoryStore trait + soft-delete on facts + Runtime::store_handle()

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- rewrite workspace-profile-baseline SDD as a static change record
- *(deps)* drop unused dependencies flagged by cargo-machete
