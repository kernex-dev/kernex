# Tasks: Memory typed row shape

> **Reference:** [proposal.md](proposal.md).
> Each task is sized to under 2 focused hours.

## Coordination rules

1. Pre-commit gate must pass before any commit: `cargo build && cargo audit && cargo deny check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo fmt --check`.
2. No `Co-Authored-By` trailers, no `--no-verify`, no auto-commit.
3. Slice A and Slice B ship as separate releases. Do not bundle them; the semver signal is the contract.

---

## Slice A — Non-breaking (0.6.1 → 0.6.2)

### A.1 Migrations fast-path

- Edit `crates/kernex-memory/src/store/mod.rs::run_migrations`.
- Replace the per-migration `SELECT name FROM _migrations WHERE name = ?` round-trip loop with one `SELECT name FROM _migrations` that loads the applied-migrations set into a `HashSet<String>`.
- Check membership in memory before the `INSERT INTO _migrations` write step. Insert path stays unchanged.
- Add a brief inline comment explaining the perf-shape (O(N²) network IO → O(N)).
- Verify: existing migration tests stay green; `cargo bench --bench cold_start` shows the cold-open cost drop.

**Status (2026-05-11):** Implemented locally at `crates/kernex-memory/src/store/mod.rs`. `cargo test --workspace` green; `cargo clippy` clean; `cargo fmt --check` clean. Not yet committed.

### A.2 `get_message_by_id` trait method

- Add `pub struct MessageRow { id: String, conversation_id: String, role: String, content: String, timestamp: String }` to `kernex-memory` and re-export from `lib.rs`. (The `timestamp` field stays as `String` in Slice A; Slice B types it as `SystemTime`.)
- Add `async fn get_message_by_id(&self, id: &str) -> Result<Option<MessageRow>, MemoryError>;` to the `MemoryStore` trait.
- Implement on `Store`: `SELECT id, conversation_id, role, content, timestamp FROM messages WHERE id = ? AND deleted_at IS NULL`. (The `deleted_at` filter is forward-compat — the messages table will gain that column when CC-9 soft-delete extends to messages; today it's a no-op.)
- Add tests: `test_get_message_by_id_happy_path`, `test_get_message_by_id_missing_returns_none`.
- Verify trait object safety remains intact (no generic methods introduced).

### A.3 Migration regression test

- Add a test that asserts `_migrations` is read in exactly one `SELECT name FROM _migrations` call (no per-migration round-trips). Easiest path: instrument a `sqlx::any::AnyKind` wrapper or use a counting connection middleware. If that's too heavy, settle for asserting `run_migrations` is idempotent + the existing round-trip count via a `tracing::debug!` event in the loop.

### A.4 Version + changelog

- Update `crates/kernex-memory/CHANGELOG.md` `[Unreleased]` with:
  - `### Added — pub fn get_message_by_id on MemoryStore + pub struct MessageRow`
  - `### Performance — fast-path migration check (single SELECT, ~10x cold-open improvement)`
- Workspace version bump: release-plz handles this on the publish run; do NOT bump manually.
- Pre-flight: confirm `cargo build --workspace` and `cargo test --workspace` are green from a clean target/.

### A.5 Commit + push

- Single atomic commit: `feat(memory): get_message_by_id trait method + migrations fast-path`.
- Push to main.
- Trigger release-plz workflow (or wait for the scheduled run).
- Confirm 0.6.2 lands on crates.io for all 8 publishable crates.

### A.6 Paired bump in kernex-agent

- After 0.6.2 publishes: bump `kernex-agent/Cargo.toml` direct deps `kernex-memory = "0.6.2"` etc.
- Wire `kx mem get` to the new `MemoryStore::get_message_by_id` trait method (Step 2.4 in `openspec/changes/kx-mem-cli-promotion/tasks.md`). This is the agent-side commit; lives in the kernex-agent repo.

---

## Slice B — Breaking (0.6.x → 0.7.0)

> Slice B ships as a separate, dedicated change. Do NOT start until Slice A is on crates.io. The breaking signal needs the semver-major bump to be visible to downstream consumers.

### B.1 Typed timestamp on `MessageRow`

- Change `MessageRow.timestamp: String` to `MessageRow.timestamp: SystemTime`.
- Update `Store::get_message_by_id` to parse the SQLite `TIMESTAMP` column via sqlx's native `chrono::DateTime<Utc>` → `SystemTime` conversion.
- Update all downstream `Store` paths that produce a `MessageRow` (currently just `get_message_by_id`).

### B.2 New `search_messages` return type

- Change `MemoryStore::search_messages` return type from `Vec<(String, String, String)>` to `Vec<MessageRow>` (same shape `get_message_by_id` returns).
- Update SQL to select id + conversation_id columns alongside role/content/timestamp.
- Update `Store::search_messages` to produce `MessageRow`s.
- Update tests + benchmarks.

### B.3 `since: Option<SystemTime>` on `search_messages`

- Add `since: Option<SystemTime>` parameter to the trait method.
- Update SQL: append `AND m.timestamp >= ?` when `since` is `Some`. Limit applies post-filter.
- Update agent-side `kx mem search` to push the parsed `--since` value through (deletes the client-side filter and `parse_sqlite_utc` / `days_from_civil`).

### B.4 New `HistoryRow` for `get_history`

- Replace `Vec<(String, String)>` return with `Vec<HistoryRow>` where `HistoryRow = { conversation_id, summary, updated_at: SystemTime }`.
- Update SQL to surface `conversation_id` alongside the existing fields.
- Update agent-side `kx mem history` to consume `updated_at: SystemTime` directly.

### B.5 Workspace version bump 0.6.x → 0.7.0

- This is automatic via release-plz once the breaking change lands. Confirm the bump is correct on the release-plz PR before merge.

### B.6 Paired kernex-agent migration

- Bump `kernex-agent/Cargo.toml` to `kernex-memory = "0.7.0"`.
- Delete `parse_sqlite_utc`, `days_from_civil`, `timestamp_after` from `src/mem/cli.rs` (~80 lines).
- Replace `parse_since` client-side filter with the new server-side `since` parameter.
- Update tests that depended on the string timestamp shape.
- Smoke-test all `kx mem *` paths.

### B.7 Spec updates

- Update `kernex-agent/openspec/changes/kx-mem-cli-promotion/spec.md` to lift the v1 limitation notes on S-search-2 (limit applies post-filter now) and S-search-3 (filter lives server-side).
- Update the agent's `cli::search` docstring to drop the "Known v1 limitation" paragraph.

---

## Definition of done

- **Slice A:** kernex-memory 0.6.2 published with `get_message_by_id` + migrations fast-path. kernex-agent main has the paired bump and `kx mem get` is functional.
- **Slice B:** kernex-memory 0.7.0 published with the typed surface. kernex-agent main has zero references to `parse_sqlite_utc` / `days_from_civil`. Spec ambiguities (S-search-2, S-search-3) closed.

Archive this change file under `openspec/archive/YYYY-MM-memory-typed-row-shape/` once Slice B lands.
