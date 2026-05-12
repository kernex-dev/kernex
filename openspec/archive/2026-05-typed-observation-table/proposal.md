# Proposal: Typed observation table

> **Change ID:** `typed-observation-table`
> **Status:** LANDED — `kernex-memory 0.8.0` published to crates.io 2026-05-12 (merged at `kernex-dev/kernex@03bebb6`, released via PR #22 squash at `cd20fca`, publish workflow run 25728229119). Workspace bumped 0.7.0 → 0.8.0 in lockstep across all 9 publishable crates.
> **Companion change:** `kernex-dev/kernex-agent/openspec/changes/kx-mem-cli-promotion/` — Step 2.11 (`kx mem save`) consumed this surface at `kernex-dev/kernex-agent@11bdf54` (PR #24 squash).

## What this change ships

A typed observation log for `kernex-memory`, accessible via five new `MemoryStore` trait methods plus a breaking refinement to `get_memory_stats`. The full surface:

1. **Migration `018_observations.sql`** — adds the `observations` base table + FTS5 mirror (`observations_fts`) with three triggers keeping the index in sync.
2. **Rust types** in `crates/kernex-memory/src/observation.rs`:
   - `SaveEntry` (input) carrying `sender_id`, typed `kind`, title, and four optional structured fields (`what` / `why` / `where` / `learned`).
   - `Observation` (output row) carrying the above plus generated `id` and `created_at` / `updated_at` as `SystemTime`.
   - `ObservationType` enum with seven variants (`Bugfix`, `Decision`, `Pattern`, `Config`, `Discovery`, `Learning`, `Architecture`), serialized in `snake_case` / `lowercase`. All three types are `#[non_exhaustive]` for forward-compat.
3. **Trait surface additions** on `MemoryStore` (5 new methods):
   - `save_observation(entry: SaveEntry) -> Result<Observation, MemoryError>`
   - `get_observation_by_id(id: &str) -> Result<Option<Observation>, MemoryError>`
   - `search_observations(query, sender_id, limit, since, kind) -> Result<Vec<Observation>, MemoryError>`
   - `soft_delete_observation(id: &str) -> Result<bool, MemoryError>`
   - `list_soft_deleted_observations(sender_id: &str) -> Result<Vec<Observation>, MemoryError>`
4. **Breaking change to `MemoryStore::get_memory_stats`** — return type goes from `Result<(i64, i64, i64), MemoryError>` to `Result<(i64, i64, i64, i64), MemoryError>` with the new tuple shape `(conversations, messages, observations, facts)`. Observation count joins at position 2; consumers must destructure four elements.

The change targets `kernex-memory 0.8.0`. Because all 9 publishable crates ship at a synchronized version, the workspace bumps in lockstep.

## Why the breaking-change bundle

Two coupled changes ship in one release rather than two:

- The trait surface extension (additive, technically non-breaking).
- The `get_memory_stats` 4-tuple (breaking).

Splitting them into two minor releases (`0.8.0` = trait additions, `0.8.1` = stats reshape) forces every downstream consumer to absorb the breaking change anyway, just one release later. Bundling lets the agent-side consumer adopt the entire new surface in one commit.

## Out of scope

- **Cross-sender / cross-DB observation queries.** Each `observations` row carries a `sender_id` only; the on-disk DB is the project scope.
- **`update_observation` / amendment.** Initial shape is append-plus-soft-delete. Editing in place is deferred; the operator path is "soft-delete the wrong row and save the corrected one".
- **`undelete_observation` recovery surface.** The trait exposes `list_soft_deleted_observations` so future tooling can offer recovery; the actual undelete write path is deferred to a follow-up change.
- **`search_observations` ranking weights.** v1 uses FTS5 `rank` ascending with a `created_at` descending tiebreaker. Weighted recency / type priority is a future tightening.

## Ratified design locks

The six locks live in [design.md](design.md) ADRs:

- **OBS-01** — Schema shape: separate `observations` table with `(id, sender_id, type, title, what, why, where_field, learned, created_at, updated_at, deleted_at)`. Soft-delete via `deleted_at` nullable column + partial indices. Mirrors the `facts` pattern from migration `017_soft_delete.sql`.
- **OBS-02** — Type column is `TEXT NOT NULL` with a SQL `CHECK` constraint covering the seven enum strings. Defense in depth: the Rust enum at the API layer plus the DB constraint at the write layer.
- **OBS-03** — `search_observations` is a separate trait method, NOT folded into `search_messages`. Messages and observations have different row shapes (messages have no `type` / `title` / structured fields), different filter semantics, and different lifecycles.
- **OBS-04** — `get_memory_stats` returns a 4-tuple `(conversations, messages, observations, facts)`. Bundled with the trait additions so one breaking release covers the full migration.
- **OBS-05** — Empty title is rejected at the DB layer with `CHECK (length(title) > 0)`. The agent's clap layer rejects empty title at the CLI parser; the DB CHECK is the second wall for any non-CLI consumer.
- **OBS-06** — Migration is forward-only. No reverse migration ships; operators recover by removing the local SQLite DB (kx is a per-developer local store, not a multi-tenant server).

## Post-merge drift

One drift from the initial design, recorded here for the audit trail:

**DRIFT-01** — Scoping moved from `project` to `sender_id`. The initial design assumed an explicit `project` column on the table and a `project` parameter on the trait methods. Pre-implementation audit found that the existing `MemoryStore` surface scopes by `sender_id` only (matching the `facts` / `messages` / `conversations` discipline); project scoping comes from the on-disk DB location (`~/.kx/projects/<name>/memory.db` in downstream CLI consumers). Observations follow the same convention: no project column, `sender_id` is the intra-DB scope. This simplifies the trait surface and aligns with the existing pattern.

## What "done" looks like

- All 5 new trait methods callable through `Arc<dyn MemoryStore>`.
- `kernex-memory 0.8.0` published on crates.io as part of the workspace cut.
- `save_observation` round-trips: a saved row is findable via `search_observations`, retrievable via `get_observation_by_id`, and counted by `get_memory_stats`.
- Soft-delete invariant: a deleted row is invisible to default reads (get, search, stats) and visible only via `list_soft_deleted_observations`.
- Migration `018_observations.sql` applies cleanly on a 0.7.x DB; second-run is a no-op via the existing `_migrations` registry fast-path.

## Workspace impact

| Crate | Change |
|---|---|
| `kernex-memory` | Migration, new types, 5 new inherent methods + matching trait methods, breaking signature on `get_memory_stats`. |
| `kernex-runtime` | None (consumes the trait via `Arc<dyn MemoryStore>`; downstream re-exports unchanged). |
| Other 7 publishable crates | Workspace version bump only. |
| `kernex-runtime/examples/full_stack.rs` | Updated to destructure the new 4-tuple from `get_memory_stats`. |
