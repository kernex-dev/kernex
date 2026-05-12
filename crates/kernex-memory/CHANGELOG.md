# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/kernex-dev/kernex/compare/v0.7.0...v0.8.0) - 2026-05-12

### Added

- *(memory)* [**breaking**] typed observations table + save_observation trait surface ([#21](https://github.com/kernex-dev/kernex/pull/21))

## [0.7.0](https://github.com/kernex-dev/kernex/compare/v0.6.2...v0.7.0) - 2026-05-11

### Added

- *(memory)* [**breaking**] typed MessageRow/HistoryRow + get_message_by_id + server-side since ([#19](https://github.com/kernex-dev/kernex/pull/19))

### Added

- *(memory)* introduce typed `MessageRow` and `HistoryRow` shapes; add `MemoryStore::get_message_by_id`; add `since: Option<SystemTime>` parameter to `MemoryStore::search_messages` (server-side recency filter, applies before `LIMIT`)

### Changed

- *(memory)* **BREAKING:** `MemoryStore::search_messages` returns `Vec<MessageRow>` (was `Vec<(String, String, String)>`); `MemoryStore::get_history` returns `Vec<HistoryRow>` (was `Vec<(String, String)>`). Both surfaces now carry `id` / `conversation_id` and parse timestamps to `SystemTime` at fetch time. Closes the breaking half of the typed-row migration (resolves the `search_messages` recency-cutoff and result-shape ambiguities visible in the downstream `kx mem *` CLI).

## [0.6.2](https://github.com/kernex-dev/kernex/compare/v0.6.1...v0.6.2) - 2026-05-11

### Performance

- Fast-path migration check in `Store::new`: replace the per-migration `SELECT name FROM _migrations WHERE name = ?` round-trip loop with one `SELECT name FROM _migrations` plus an in-memory `HashSet` lookup. O(N²) network IO becomes O(N); saves ~10 ms per cold open on warm cache.

### Documentation

- Add `openspec/changes/memory-typed-row-shape/` (proposal + tasks) that locks the two-stage plan for typed row shapes on the trait surface. This release ships the perf fast-path; the breaking stage (deferred to 0.7.0) types `updated_at` as `SystemTime`, pushes `since: Option<SystemTime>` server-side into `search_messages`, and surfaces `MessageRow` / `HistoryRow` from the trait return.

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
