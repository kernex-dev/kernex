# Spec: Typed observation table

> **Reference:** [proposal.md](proposal.md), [design.md](design.md).
> Behavioral scenarios for the `MemoryStore` trait surface using Given / When / Then. Verified by the in-tree `#[cfg(test)] mod tests` block in `crates/kernex-memory/src/store/tests.rs`.

## Cross-cutting invariants

### M-obs-CC-1 Soft-delete is invisible by default

- **Given** an observation with `deleted_at IS NOT NULL`
- **When** `search_observations`, `get_observation_by_id`, or `get_memory_stats` is invoked
- **Then** the row is excluded.
- **Note:** only `list_soft_deleted_observations` surfaces deleted rows.

### M-obs-CC-2 Type values are constrained at the DB layer

- **Given** any attempt to write an observation
- **When** the `type` value is not one of the seven enum strings
- **Then** the write fails with `MemoryError::Sqlite` carrying the CHECK constraint violation; no row is persisted.

### M-obs-CC-3 Empty title is rejected at the DB layer

- **Given** any attempt to write an observation
- **When** `title` is `""`
- **Then** the write fails with `MemoryError::Sqlite` carrying the `CHECK (length(title) > 0)` violation.

### M-obs-CC-4 FTS index reflects soft-delete

- **Given** an observation O that is searchable
- **When** O is soft-deleted via `soft_delete_observation`
- **Then** subsequent `search_observations` calls do NOT return O. The `observations_au` trigger removes the FTS row.

### M-obs-CC-5 IDs are UUIDv4 strings, never reused

- **Given** any successful `save_observation`
- **When** the returned `Observation::id` is inspected
- **Then** it is a UUIDv4 in standard hyphenated form, and no two distinct observations share an id.

---

## `MemoryStore::save_observation`

### M-obs-save-1 Happy path round-trip

- **Given** a fresh `Store`
- **When** `save_observation(entry)` with all 7 fields populated is called
- **Then** the returned `Observation` carries a fresh `id`, identical structured fields, and `created_at == updated_at` set to "now".
- **And** `get_observation_by_id(returned.id)` returns `Some(returned)` (modulo timestamp precision).

### M-obs-save-2 Save with None optionals

- **Given** a fresh `Store`
- **When** `save_observation` is called with `what / why / where_field / learned` all `None`
- **Then** the row persists with NULL in those four columns.
- **And** the row is findable by `search_observations(title)` because the FTS column indexes `title`.

### M-obs-save-3 Save rejects empty title

- **Given** a fresh `Store`
- **When** `save_observation` is called with `title: ""`
- **Then** the call returns `Err(MemoryError::Sqlite { .. })` and no row is persisted.

### M-obs-save-4 Save rejects unknown type at the DB layer

- **Given** a fresh `Store`
- **When** a raw INSERT bypassing `ObservationType` writes `type = 'bogus'`
- **Then** the call returns `Err(_)` from the SQL CHECK constraint.

---

## `MemoryStore::get_observation_by_id`

### M-obs-get-1 Happy path returns full record

- **Given** an observation persisted at id X
- **When** `get_observation_by_id(X)` is called
- **Then** the result is `Ok(Some(Observation))` with all fields matching.

### M-obs-get-2 Missing id returns None

- **When** `get_observation_by_id("not-a-real-id")` is called
- **Then** the result is `Ok(None)`.

### M-obs-get-3 Soft-deleted id returns None

- **Given** an observation at id X with `deleted_at` set
- **When** `get_observation_by_id(X)` is called
- **Then** the result is `Ok(None)`. Verifies M-obs-CC-1.

---

## `MemoryStore::search_observations`

### M-obs-search-1 Happy path FTS match

- **Given** an observation whose title or any structured field contains `"N+1"`
- **When** `search_observations(query="N+1", sender_id, limit=10, since=None, kind=None)` is called
- **Then** the result contains the matching observation.

### M-obs-search-2 `kind` filter narrows

- **Given** observations of types `Bugfix` and `Decision` both matching a query
- **When** `search_observations(..., kind=Some(Bugfix), ...)` is called
- **Then** only the `Bugfix` observation is returned.

### M-obs-search-3 `since` filters by recency

- **Given** matching observations
- **When** `search_observations(..., since=Some(<future>), ...)` is called
- **Then** all rows are filtered out (none exist after the future cutoff).

### M-obs-search-4 `sender_id` scope is hard

- **Given** matching observations under sender_ids `alice` and `bob`
- **When** `search_observations(..., sender_id="alice", ...)` is called
- **Then** only the `alice` observation is returned.

### M-obs-search-5 Empty corpus returns empty Vec

- **Given** a `Store` with zero observations
- **When** any `search_observations(...)` is called
- **Then** the result is `Ok(vec![])`.

---

## `MemoryStore::soft_delete_observation`

### M-obs-softdelete-1 Soft-delete updates `deleted_at`

- **Given** an active observation at id X
- **When** `soft_delete_observation(X)` is called
- **Then** the result is `Ok(true)` and the row's `deleted_at` becomes non-NULL.

### M-obs-softdelete-2 Idempotent (false on second call)

- **Given** an observation at id X that is already soft-deleted
- **When** `soft_delete_observation(X)` is called again
- **Then** the result is `Ok(false)`.

### M-obs-softdelete-3 Missing id returns false

- **When** `soft_delete_observation("not-a-real-id")` is called
- **Then** the result is `Ok(false)`. Not an error; a no-op.

---

## `MemoryStore::list_soft_deleted_observations`

### M-obs-listdeleted-1 Returns only soft-deleted rows

- **Given** a sender with both active and soft-deleted observations
- **When** `list_soft_deleted_observations(sender_id)` is called
- **Then** the result contains exactly the soft-deleted rows, ordered by `deleted_at` descending.

### M-obs-listdeleted-2 Sender scope is enforced

- **Given** sender_ids `alice` and `bob` each with one soft-deleted observation
- **When** `list_soft_deleted_observations("alice")` is called
- **Then** the result contains only the `alice` row.

---

## `MemoryStore::get_memory_stats` (breaking signature change)

### M-obs-stats-1 4-tuple ordering

- **Given** a sender with 1 conversation, 3 messages, 2 observations, 5 facts
- **When** `get_memory_stats(sender_id)` is called
- **Then** the result is `Ok((1, 3, 2, 5))`.

### M-obs-stats-2 Soft-deleted observations are excluded

- **Given** a sender with 2 active and 3 soft-deleted observations
- **When** `get_memory_stats(sender_id)` is called
- **Then** the third element of the tuple is `2`. Verifies M-obs-CC-1 against the stats surface.

---

## Migration scenarios

### M-obs-mig-1 Migration applies cleanly on a 0.7.x DB

- **Given** a SQLite database at the 0.7.x schema (migrations 001-017 applied)
- **When** `Store::run_migrations` runs
- **Then** migration 018 applies successfully. The `observations` table, `observations_fts` virtual table, and 3 triggers all exist. `_migrations` records `018_observations` as applied.

### M-obs-mig-2 Migration is idempotent

- **Given** a database that already has migration 018 applied
- **When** `Store::run_migrations` runs again
- **Then** the registry fast-path short-circuits; the migration SQL does not re-execute. Verifiable by `SELECT COUNT(*) FROM observations` returning the unchanged count after the second run.

---

## Out of scope (NOT covered by this spec)

- **JSON wire format / exit codes / sandbox refusal.** Consumer-layer concerns. Owned by the agent-side `kx-mem-cli-promotion` change.
- **`search_observations` cold-start bench.** Informational measurement; tracked separately.
- **`undelete_observation` write path.** Not in v1; recovery via `list_soft_deleted_observations` plus future tooling.
