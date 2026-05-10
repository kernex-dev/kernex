# Tasks: Memory store trait introduction

> **Reference:** [proposal.md](proposal.md). Each task is sized at roughly two focused hours. Change tag: `[s4-d]`.
> **Status:** Archived. Landed at `kernex-dev/kernex@d9fc777` on 2026-05-10. See `proposal.md` post-merge notes for drifts.

---

## Step 0 — Pre-execution audit `[s4-d]`

Verify the ground state before authoring any code.

### P0-1. Confirm precedent changes are archived `[s4-d]`

- Confirm `openspec/archive/` contains a directory matching `*workspace-profile-baseline*`
- Confirm `openspec/archive/2026-05-workspace-crate-split/` exists and contains `proposal.md` + `tasks.md`
- Read both archived `tasks.md` files to confirm they shipped to completion (no open checkboxes)

**Verification:** `ls openspec/archive/` lists both. `grep -n '\[ \]' openspec/archive/2026-05-workspace-crate-split/tasks.md` returns no matches.

### P0-2. Confirm baseline build is clean `[s4-d]`

- `cargo build --workspace --all-targets` succeeds
- `cargo test --workspace` passes
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all --check` clean

**Verification:** all four commands exit 0 on a freshly cloned `main`.

### P0-3. Enumerate the 14 trait-candidate methods from current `Store` source `[s4-d]`

- Open `crates/kernex-memory/src/store/mod.rs` and inner modules under `crates/kernex-memory/src/store/`
- Confirm each of the 14 methods listed in the proposal exists today on `impl Store`:
  - `close_current_conversation`
  - `get_memory_stats`
  - `db_size`
  - `get_total_usage`
  - `get_facts`
  - `delete_fact`
  - `get_history`
  - `search_messages`
  - `create_task`
  - `get_tasks_for_sender`
  - `complete_task`
  - `fail_task`
  - `cancel_task`
  - `get_due_tasks`
- Capture each method's exact signature (return type, parameters) into a working note for Step 1

**Verification:** `grep -nE 'pub (async )?fn (close_current_conversation|get_memory_stats|db_size|get_total_usage|get_facts|delete_fact|get_history|search_messages|create_task|get_tasks_for_sender|complete_task|fail_task|cancel_task|get_due_tasks)\b' crates/kernex-memory/src/store/` returns exactly 14 hits with one match per method name.

### P0-4. Confirm migration numbering and `facts` schema `[s4-d]`

- `ls crates/kernex-memory/migrations/` shows files `001_init.sql` through `016_cache_token_breakdown.sql`; the next slot is `017`
- Inspect `001_init.sql` (or the latest migration touching `facts`) to confirm the current `facts` columns: `id, sender_id, key, value, source_message_id, created_at, updated_at` with `UNIQUE(sender_id, key)`. Confirm no `deleted_at` exists.
- Inspect the migration registration site at `crates/kernex-memory/src/store/mod.rs:223-285` to confirm the format of the existing list (so the new entry matches conventions).

**Verification:** `ls crates/kernex-memory/migrations/ | tail -1` reports `016_cache_token_breakdown.sql`. `grep -n deleted_at crates/kernex-memory/migrations/*.sql` returns no matches.

### P0-5. Confirm `MemoryError` shape `[s4-d]`

- Read `crates/kernex-memory/src/error.rs:9-49` and confirm variants `Sqlite`, `Io`, `Serde`, `Logic` are present.
- Confirm `MemoryError` is `pub` and re-exported through `lib.rs`.

**Verification:** `grep -nE 'pub enum MemoryError|Sqlite|Io|Serde|Logic' crates/kernex-memory/src/error.rs` shows the four variants in the listed range.

### P0-6. Capture cold-start baseline `[s4-d]`

- Run `cargo bench --bench cold_start` (or the workspace equivalent) to capture the current `bench_memory_search_cold_start` measurement.
- Record the median and p95 in a working note. Expected range based on prior runs: 1.87–1.94 ms.

**Verification:** the bench output shows `bench_memory_search_cold_start` with a median in the expected range. This is the baseline the rewired bench in Step 4 must not regress past 50 ms p95.

---

## Step 1 — Author the trait `[s4-d]`

### P1-1. Create `crates/kernex-memory/src/store/trait.rs` `[s4-d]`

- New file `crates/kernex-memory/src/store/trait.rs`
- Declare `pub trait MemoryStore: Send + Sync` with the 14 public method signatures captured in P0-3, plus the three soft-delete methods:
  - `soft_delete_fact`
  - `soft_delete_facts`
  - `list_soft_deleted_facts`
- All methods return `Result<_, MemoryError>`
- Method signatures take `&self` (or `&mut self` only where the existing inherent method requires it; aim for `&self` everywhere with interior mutability handled by the implementor)
- If any method is async on the inherent impl, mark the trait method `async` and add `#[async_trait::async_trait]` consistent with workspace conventions

**Verification:** `cargo build -p kernex-memory` compiles. `cargo doc -p kernex-memory --no-deps` renders the trait page with all 17 methods.

### P1-2. Re-export and refresh crate doc comment `[s4-d]`

- In `crates/kernex-memory/src/store/mod.rs`, add `pub mod r#trait;` (or `mod trait_def;` with a `pub use` if `trait` is reserved in the path)
- In `crates/kernex-memory/src/lib.rs`, add `pub use store::r#trait::MemoryStore;` (or matching path)
- Update the stale top-of-file doc comment at `crates/kernex-memory/src/lib.rs:1` to accurately describe the crate now that the trait surface exists. The comment should mention both the concrete `Store` and the abstract `MemoryStore` trait.

**Verification:** `cargo build -p kernex-memory` clean. `grep -n 'pub use.*MemoryStore' crates/kernex-memory/src/lib.rs` shows the re-export. `cargo doc -p kernex-memory --no-deps` renders the updated crate-level doc.

---

## Step 2 — Implement and soft-delete `[s4-d]`

### P2-1. `impl MemoryStore for Store` `[s4-d]`

- In a new module `crates/kernex-memory/src/store/trait_impl.rs` (or inline at the bottom of `store/mod.rs` — pick whichever matches existing conventions for the crate), add `impl MemoryStore for Store { ... }`
- Each trait method forwards to the matching inherent method on `Store`
- No business logic is duplicated; the impl is a pure forwarding layer for the existing 14 methods
- Soft-delete methods (`soft_delete_fact`, `soft_delete_facts`, `list_soft_deleted_facts`) are implemented here as new logic; see P2-4

**Verification:** `cargo build -p kernex-memory --all-targets` clean. `cargo clippy -p kernex-memory --all-targets -- -D warnings` clean.

### P2-2. Author `017_soft_delete.sql` migration `[s4-d]`

- New file `crates/kernex-memory/migrations/017_soft_delete.sql`
- Add `deleted_at TEXT` column to `facts` (nullable, default `NULL`)
- Add a partial index on the live-row read path: `CREATE INDEX IF NOT EXISTS idx_facts_active ON facts (sender_id, key) WHERE deleted_at IS NULL;`
- Migration is idempotent (`ALTER TABLE ... ADD COLUMN` is naturally idempotent on first run; the index uses `IF NOT EXISTS`)

**Verification:** `sqlite3 :memory: < <(cat crates/kernex-memory/migrations/00*.sql crates/kernex-memory/migrations/01*.sql)` runs cleanly. `PRAGMA table_info(facts);` shows the new `deleted_at` column.

### P2-3. Default-hide reads `[s4-d]`

Update each of these existing methods on `Store` to filter `WHERE deleted_at IS NULL`:

- `Store::store_fact` (UPSERT path: ensure conflict resolution accounts for soft-deleted rows; an UPSERT against a soft-deleted row should clear `deleted_at` to revive the row, OR insert a new row — pick the behaviour that matches the `UNIQUE(sender_id, key)` constraint and document it inline)
- `Store::get_fact`
- `Store::get_facts`
- `Store::get_all_facts`
- `Store::get_all_facts_by_key`
- `Store::is_new_user`

**Verification:** `grep -nE 'FROM facts' crates/kernex-memory/src/store/` shows every read site, and each one either filters `deleted_at IS NULL` or is one of the soft-delete listing methods (`list_soft_deleted_facts`) or one of the hard-delete tooling methods. Add a unit test that asserts a soft-deleted fact is invisible to `get_fact` and `get_facts`.

### P2-4. Soft-delete trait methods `[s4-d]`

- `soft_delete_fact(&self, sender_id, key) -> Result<(), MemoryError>`: `UPDATE facts SET deleted_at = ? WHERE sender_id = ? AND key = ? AND deleted_at IS NULL`
- `soft_delete_facts(&self, sender_id) -> Result<u64, MemoryError>`: `UPDATE facts SET deleted_at = ? WHERE sender_id = ? AND deleted_at IS NULL`, returns rows affected
- `list_soft_deleted_facts(&self, sender_id) -> Result<Vec<Fact>, MemoryError>`: `SELECT ... FROM facts WHERE sender_id = ? AND deleted_at IS NOT NULL`
- All three methods are on the trait AND on the inherent `Store` impl (forwarding pattern matches the rest of the trait)

**Verification:** `cargo test -p kernex-memory` includes new tests covering: soft-delete then list returns the row; soft-delete then `get_fact` returns `None`; bulk soft-delete returns the correct count.

### P2-5. Hard-delete stays off the trait (for `delete_facts`) `[s4-d]`

- Confirm `delete_fact` is on the trait (per the 14-method list in the proposal)
- Confirm `delete_facts` (bulk hard delete) is **NOT** on the trait — it stays as an inherent `Store` method only, for emergency cleanup tooling that has direct concrete access
- Add a doc comment on `Store::delete_facts` noting it is intentionally not on the `MemoryStore` trait

**Verification:** `grep -n 'fn delete_facts' crates/kernex-memory/src/store/trait.rs` returns no match. `grep -n 'fn delete_facts' crates/kernex-memory/src/store/` returns exactly one match (the inherent impl).

### P2-6. Register migration in the migration list `[s4-d]`

- Open `crates/kernex-memory/src/store/mod.rs:223-285` and add `017_soft_delete.sql` to the registered migration list, matching the conventions of the existing entries (file path, embed macro, ordering)

**Verification:** `cargo test -p kernex-memory` runs migrations end-to-end on a fresh in-memory SQLite and reports the `facts` table now has a `deleted_at` column.

---

## Step 3 — Runtime exposure `[s4-d]`

### P3-1. Add `Runtime::store_handle()` `[s4-d]`

- In `crates/kernex-runtime/src/...` (the file that declares `pub struct Runtime`), add a new method `pub fn store_handle(&self) -> Arc<dyn MemoryStore>`
- The method returns an `Arc<dyn MemoryStore>` cloned/derived from the Runtime's existing `Store` field. If the existing `runtime.store` is owned by value, wrap it once at Runtime construction time into an `Arc<dyn MemoryStore>` and store the handle alongside the existing field.
- The existing `runtime.store: Store` public field stays unchanged (backwards compat). Do NOT mark it `#[deprecated]` in this change; deprecation is a follow-up change.
- Re-export `MemoryStore` from `kernex-runtime` so consumers binding through Runtime do not need a direct dep on `kernex-memory` if they only need the trait

**Verification:** `cargo build -p kernex-runtime` clean. `cargo doc -p kernex-runtime --no-deps` shows `Runtime::store_handle` and the re-exported `MemoryStore`. A new unit test asserts the returned handle responds correctly to one trait method (e.g., `get_memory_stats`) against a freshly initialised Runtime.

---

## Step 4 — Bench harness rewire `[s4-d]`

### P4-1. Rewire `bench_memory_search_cold_start` through the trait `[s4-d]`

- Open `bench/benches/cold_start.rs`
- Locate `bench_memory_search_cold_start`. It currently calls `Store::search_messages` directly on the concrete struct.
- Change the bound site so the bench dispatches through `&dyn MemoryStore::search_messages`. Pattern: bind a `let store: &dyn MemoryStore = &concrete_store;` (or `let store: Arc<dyn MemoryStore> = ...;`) and call through that binding inside the iter loop.
- Run `cargo bench --bench cold_start` and record the new measurement.
- Confirm p95 stays under 50 ms (today's median is 1.87–1.94 ms; trait dispatch overhead must not push p95 over the gate).

**Verification:** `cargo bench --bench cold_start` runs to completion and reports `bench_memory_search_cold_start` with p95 < 50 ms. Capture the new median in the change notes for archive.

---

## Step 5 — Verification gate `[s4-d]`

### P5-1. Workspace build and test `[s4-d]`

- `cargo build --workspace --all-targets`
- `cargo test --workspace`

**Verification:** both exit 0.

### P5-2. Lint and format `[s4-d]`

- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`

**Verification:** both exit 0.

### P5-3. Supply chain `[s4-d]`

- `cargo audit`
- `cargo deny check`
- `cargo machete`

**Verification:** all three exit 0 (or only flag pre-existing entries already accepted on `main`).

### P5-4. Doc round-trip on the new public symbols `[s4-d]`

- `cargo doc --workspace --no-deps`
- Open the rendered docs and confirm:
  - `kernex-memory::MemoryStore` trait page lists all 17 methods with rustdoc comments
  - `kernex-memory::Store` page shows `impl MemoryStore for Store`
  - `kernex-runtime::Runtime::store_handle` is rendered with its return type
  - The refreshed `kernex-memory` crate-level doc comment is rendered correctly

**Verification:** `cargo doc --workspace --no-deps` exits 0 with no broken intra-doc links. Manual visual check of the four points above.

### P5-5. Bench gate confirmation `[s4-d]`

- Re-run `cargo bench --bench cold_start` one final time on the merge candidate
- Confirm `bench_memory_search_cold_start` p95 < 50 ms
- Capture the final number in the archive notes

**Verification:** numerical p95 captured and below the gate.

### P5-6. Default-hide grep audit `[s4-d]`

- `grep -nE 'FROM facts' crates/kernex-memory/src/store/` enumerates every read site
- For each hit, confirm one of:
  - The query filters `WHERE deleted_at IS NULL`
  - The query is a soft-delete listing path (`list_soft_deleted_facts`)
  - The query is a hard-delete tooling path (`delete_fact`, `delete_facts`)
  - The query is a soft-delete write path (`soft_delete_fact`, `soft_delete_facts`)

**Verification:** every `FROM facts` hit accounted for. No silent reader leaks soft-deleted rows.

---

## Step 6 — Archive and post-merge `[s4-d]`

### P6-1. Move the change directory to archive `[s4-d]`

- After merge, move `openspec/changes/memory-store-trait-introduction/` to `openspec/archive/2026-MM-memory-store-trait-introduction/` (substitute the actual landing month in `MM`)
- Confirm both `proposal.md` and `tasks.md` carry over with completed checkboxes
- Update any documentation map or index file that references the in-flight path

**Verification:** `ls openspec/archive/ | grep memory-store-trait-introduction` returns the archived directory. `ls openspec/changes/memory-store-trait-introduction/` returns no such directory (or no longer exists).

### P6-2. Capture the trait surface for downstream consumers `[s4-d]`

- Append a short note to the archived `proposal.md` recording the final shipped trait method count and the final `bench_memory_search_cold_start` measurement
- Confirm a published version of `kernex-runtime` re-exports `MemoryStore`, so the sister-repo follow-up change has a stable surface to bind against

**Verification:** archived proposal contains the final numbers. `cargo doc -p kernex-runtime --no-deps` on the published version shows the re-exported trait.

---

## Done criteria

- All checkboxes above ticked
- `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`, `cargo audit`, `cargo deny check`, `cargo machete`, `cargo doc` all clean on the merge candidate
- `bench_memory_search_cold_start` p95 < 50 ms with trait dispatch on the measured path
- No existing call site changed signature; no error type renamed; `runtime.store` field still public
- Change directory archived under `openspec/archive/2026-MM-memory-store-trait-introduction/`
