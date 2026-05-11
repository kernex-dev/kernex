# Proposal: Memory typed row shape

- **Status:** Pending
- **Author:** Jose Hurtado
- **Estimated effort:** ~5 working days, split across two slices (A + B)
- **Repo:** `kernex-dev/kernex`
- **Change ID:** `memory-typed-row-shape`
- **Change tag:** `[s5-d]` (Sprint 5, kernex-memory)

## Operator friction

The `kernex-memory 0.6.1` trait surface returns timestamps as raw `String` values from `MemoryStore::search_messages` and `MemoryStore::get_history`. The sister repo's binary (`kernex-dev/kernex-agent`) consumes these from its `kx mem *` CLI subcommands and has to hand-roll a fixed-width parser (`parse_sqlite_utc` + `days_from_civil`, ~80 lines in `src/mem/cli.rs`) to support `kx mem search --since 30d` recency filtering client-side. Three concrete consequences:

1. **The hand-rolled parser is fragile.** It accepts the SQLite `CURRENT_TIMESTAMP` shape (`YYYY-MM-DD HH:MM:SS`) and the ISO-8601 variant. Any future migration that switches to millisecond precision or numeric epoch silently mis-filters every `--since` query downstream; no compile-time signal alerts.

2. **The `--limit` semantic mismatches the spec.** `kx-mem-cli-promotion` spec scenario S-search-2 says "limit caps the result count" but the current `search_messages` signature applies the limit pre-filter (at the FTS5 layer), then the binary trims again client-side after `--since` / `--type`. A query like `kx mem search foo --limit 10 --since 30d` returns fewer than 10 records even when more would qualify. Pushing `since` server-side into the SQL `WHERE` clause makes the limit apply post-filter.

3. **No `get_message_by_id` surface.** `kx mem get <id>` has no trait method to wire to. Phase D-agent Step 2.4 ships today as `CliError::NotImplemented` and the closure trigger waits on this change.

A second, smaller friction: `Store::new` runs migration-existence checks via one `SELECT name FROM _migrations WHERE name = ?` per migration. With 17 migrations the cold-open cost is 17 round-trips on every binary invocation (`kx dev`, `kx mem *`, `kx audit`, etc.). On warm cache this is ~10 ms; not a regression, but a perf paper-cut that scales linearly with migration count.

## Solution overview

Two coordinated slices. Slice A is pure-additive and ships in a minor bump (0.6.x → 0.6.2). Slice B is breaking and ships in a major bump (0.6.x → 0.7.0) so semver consumers get a compile-time signal to migrate.

### Slice A — Non-breaking (0.6.2 candidate)

Two changes that consumers can adopt opportunistically:

1. **Migrations fast-path in `Store::new`.** Replace the per-migration `SELECT name FROM _migrations WHERE name = ?` round-trip loop with one `SELECT name FROM _migrations` that loads the applied-migrations set into a `HashSet<String>`, then check membership in memory. O(N²) network IO becomes O(N). Drops the cold-open cost from ~10 ms to ~1 ms on warm cache.

2. **New `MemoryStore::get_message_by_id`** method. Returns `Option<MessageRow>` where `MessageRow = { id, conversation_id, role, content, timestamp }`. Pure addition; no existing call site changes. Binary embedders that need to fetch a single message by id (the agent's pending `kx mem get` handler) now have a trait to wire to.

The trait method signature change is purely additive (default trait methods are not used; consumers that already `impl MemoryStore for Store` are not affected since `Store` is the only impl in this workspace).

### Slice B — Breaking (0.7.0 major bump)

Three coordinated changes to the existing `search_messages` and `get_history` methods. All three are semver-major because they change return types and parameter types on public trait methods:

1. **Typed `updated_at: SystemTime` in return tuples.** `search_messages` returns `Vec<(MessageRow)>` (the same shape `get_message_by_id` returns) instead of `Vec<(String, String, String)>` of `(role, content, timestamp)`. `get_history` returns `Vec<HistoryRow>` where `HistoryRow = { conversation_id, summary, updated_at: SystemTime }` instead of `Vec<(String, String)>`. The agent's `parse_sqlite_utc` + `days_from_civil` helpers delete cleanly.

2. **Push `since` server-side.** `search_messages` gains an `since: Option<SystemTime>` parameter and the SQL adds `AND m.timestamp >= ?`. The limit applies post-filter naturally. Resolves S-search-2 spec ambiguity.

3. **Optional fourth change: typed message `id`.** Today the messages table uses a UUID string column for `id`. Surface this through the trait return so binary embedders no longer synthesize fake stable ids (the agent currently uses `msg-{rank}-{timestamp}` for display). No schema change required; just expose the existing column.

Slice B leaves `Store`'s inherent methods alone (they keep their `String`-returning signatures for backward compat within the crate's tests). Only the trait surface changes.

## Scope (in scope)

**Slice A (this change, lands at 0.6.2):**

- `Store::run_migrations` fast-path refactor in `kernex-memory/src/store/mod.rs`.
- New `MemoryStore::get_message_by_id` method on the trait + `Store` impl.
- New `MessageRow` struct exported from `kernex-memory`.
- Regression tests: one for migration idempotence (the existing tests already cover the round-trip, but a perf-shape assertion locks the single-query path).
- One test for `get_message_by_id` happy path + soft-deleted exclusion (CC-9 for the agent's downstream `kx mem get`).
- No `CHANGELOG.md` entry yet (release-plz drives this).

**Slice B (deferred; tracked separately under this change ID):**

- Breaking signature changes on `search_messages` and `get_history`.
- Typed `MessageRow` / `HistoryRow` return types.
- `since: Option<SystemTime>` parameter on `search_messages` + corresponding SQL.
- Workspace version bump 0.6.x → 0.7.0.
- Coordinated kernex-agent bump to consume the new trait surface.
- Spec/SDD updates to lock the new shapes.

## Out of scope

- The typed observation table (with `type`, `title`, `what`, `why`, `where`, `learned` columns). That schema unblocks `kx mem save` (Phase D-agent Step 2.11) and the full S-get-1 spec compliance for `kx mem get`. Tracked as a separate future change; not coupled to this row-shape work.
- Backend pluggability (postgres, libsql). The trait is already abstract enough.
- Per-sender connection pooling. Single pool per `Store` stays unchanged.

## Migration path

**For binary embedders (`kernex-agent` and any future consumers):**

- Slice A (0.6.2): bump `kernex-memory` dep, optionally wire `kx mem get` to the new `get_message_by_id` method. No required call-site changes for existing search/history paths.
- Slice B (0.7.0): bump dep, replace `parse_sqlite_utc` and `days_from_civil` with the trait's typed `updated_at` field, replace `parse_since` client-side filter with the new server-side `since` parameter. Compile errors guide every call site that needs updating.

**For external consumers:** None known. `kernex-memory` is a workspace-internal trait surface; the published crate is consumed by `kernex-agent` and any third-party binary that has adopted the 0.6.x crate. The 0.7.0 bump signals the breaking change cleanly.

## Verification

- `cargo test --workspace`: green at the gate.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- Cold-open benchmark (`bench/benches/cold_start.rs`): re-run after Slice A to confirm the fast-path drops the migration sweep cost; lock the new baseline.
- One new regression test asserting `_migrations` is read in a single SELECT (asserts query count via tracing or instrumented pool wrapper).

## References

- Spec scenarios driving this change live in `kernex-dev/kernex-agent/openspec/changes/kx-mem-cli-promotion/spec.md` (S-search-2, S-get-1, S-get-2, S-get-3).
- The agent-side hand-rolled parser this change deletes: `kernex-agent/src/mem/cli.rs` (`parse_sqlite_utc`, `days_from_civil`, `parse_since`, `timestamp_after`).
- Original Phase D-runtime change that introduced the trait surface: `openspec/archive/2026-05-memory-store-trait-introduction/`.
