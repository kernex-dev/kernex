# Proposal: Memory store trait introduction

- **Status:** Draft v0.1
- **Author:** Jose Hurtado
- **Estimated effort:** ~10 working days
- **Repo:** `kernex-dev/kernex`
- **Change ID:** `memory-store-trait-introduction`
- **Change tag:** `[s4-d]`

## Operator friction

Today `kernex-memory` exposes a single concrete public surface: `pub struct Store` (defined at `crates/kernex-memory/src/store/mod.rs:39`). Zero public traits exist in the crate. The workspace re-exports `Store` through `kernex-runtime`, and every downstream consumer reaches into that concrete struct directly:

- The sister-repo binary (`kernex-dev/kernex-agent`) drives its REPL slash commands by calling concrete `Store` methods.
- A future CLI subcommand surface in the sister repo has a `memory-cli` cfg slot reserved by the previously landed `cargo-feature-graph` change, but there is no trait surface to bind that slot against.
- A possible HTTP API or MCP shim later would also need to share the Runtime's composed `Store` instance instead of opening a second SQLite connection against the same database file.

Without a trait surface, every new consumer is glued to the concrete shape of `Store`. Internal refactors (column renames, query restructuring, an eventual second backend) ripple straight into every caller. There is no abstraction seam between "what consumers can ask the memory layer to do" and "how the SQLite-backed store implements it today."

A second concrete friction: soft-delete is absent. The current `delete_fact` and `delete_facts` methods issue raw `DELETE FROM facts ...` statements. There is no recovery path if a user (or an agent acting on a user's behalf) deletes a fact in error. The `facts` table schema today is `(id PK, sender_id, key, value, source_message_id, created_at, updated_at, UNIQUE(sender_id, key))`; there is no `deleted_at` column. The crate has shipped 16 migrations under a 3-digit numbering scheme (`001_init.sql` through `016_cache_token_breakdown.sql`); the next slot is `017`.

## Solution overview

Introduce a `pub trait MemoryStore: Send + Sync` in `kernex-memory` that mirrors the existing public method surface that downstream consumers actually call today. Implement the trait for the existing `Store`. Add soft-delete to the `facts` table. Re-export the trait through `kernex-runtime` and expose a `Runtime::store_handle() -> Arc<dyn MemoryStore>` accessor so future binary consumers can share the Runtime's composed instance.

The change is pure-additive at the type level: `Store` keeps its inherent methods, the existing `runtime.store: Store` field stays, and no error type is renamed. After this change lands, the sister repo's reserved `memory-cli` cfg slot has a trait to bind against in a sister-repo follow-up change.

## Scope (in scope)

1. **Trait surface** in `kernex-memory`. Define `pub trait MemoryStore: Send + Sync` mirroring the 14 public methods that downstream consumers call today:
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

   Plus three new soft-delete trait methods:
   - `soft_delete_fact`
   - `soft_delete_facts`
   - `list_soft_deleted_facts`

2. **Implement `MemoryStore` for `Store`**. The existing concrete `Store` struct gains an `impl MemoryStore for Store` block. All current inherent methods stay in place; the trait impl forwards to them.

3. **Soft-delete migration**. New file `crates/kernex-memory/migrations/017_soft_delete.sql` adds a `deleted_at TEXT` column to `facts` (nullable, default `NULL`) plus a partial index `WHERE deleted_at IS NULL` to keep default-read paths fast. Registered in the migration list at `crates/kernex-memory/src/store/mod.rs:223-285`.

4. **Default-hide reads**. Update `Store::store_fact`, `get_fact`, `get_facts`, `get_all_facts`, `get_all_facts_by_key`, and `is_new_user` to filter `WHERE deleted_at IS NULL`. Soft-deleted rows are recoverable via `list_soft_deleted_facts` but invisible to ordinary reads.

5. **Hard-delete stays off the trait**. The existing `delete_fact` and `delete_facts` methods remain on the `Store` inherent impl and are reachable through the trait for `delete_fact` (per the listed 14 methods), but the bulk hard-delete `delete_facts` is intentionally NOT promoted to the trait. It stays a `Store`-only method for emergency cleanup tooling that has direct access to the concrete struct.

6. **Runtime exposure**. Add `Runtime::store_handle() -> Arc<dyn MemoryStore>` in `kernex-runtime`. The existing `runtime.store: Store` public field stays in place for backwards compatibility. Deprecating the direct field is deferred to a follow-up change.

7. **Bench harness rewire**. The existing `bench_memory_search_cold_start` benchmark at `bench/benches/cold_start.rs` measures cold-start `search_messages` against the underlying `Store` direct call (1.87–1.94 ms today). Rewire it to dispatch through `&dyn MemoryStore::search_messages` so the bench validates the trait dispatch path going forward. No new bench targets are added.

8. **Doc-comment refresh**. The stale crate-level doc comment at `crates/kernex-memory/src/lib.rs:1` is updated to accurately describe the public surface now that the trait exists.

## Scope (out of scope)

- **No concrete sister-repo work.** This change is single-repo `kernex-dev/kernex` only. The sister repo (`kernex-dev/kernex-agent`) binding its `memory-cli` cfg slot against the new trait is a sister-repo follow-up change.
- **No REPL refactor.** Any refactor of the sister-repo binary's REPL slash commands to delegate to a unified rendering layer is the sister repo's concern and is deferred to a sister-repo follow-up change.
- **No `StoreError` rename.** The crate keeps `MemoryError`. The trait surface returns `Result<_, MemoryError>` everywhere. Variants today (`Sqlite`, `Io`, `Serde`, `Logic`, defined at `crates/kernex-memory/src/error.rs:9-49`) are unchanged.
- **No new `observations` table.** Schema additions are limited to the `deleted_at` column on `facts`.
- **No additional bench targets.** Only the existing `bench_memory_search_cold_start` is rewired.
- **No concrete adapter implementations.** A second `MemoryStore` implementor (e.g., an in-memory test double or an alternative backend) is not part of this change. The trait is shaped to allow one in the future, but the only implementor introduced here is the existing `Store`.
- **No deprecation of the `runtime.store` field.** Stays addressable as `Store` for backwards compat. Deprecation is a follow-up change.

## Why this scope

This is the foundation layer for every later consumer that needs to share or substitute the memory backend. The change is intentionally small and pure-additive:

- **Foundation.** A trait surface is the prerequisite for any future second implementor (test doubles, alternative backends, remote shims) and for any consumer that wants to depend on the abstract shape rather than the concrete struct.
- **Low risk.** No existing call site changes signature. `Store` keeps its inherent methods. The `runtime.store` field stays public. The error type is unchanged. Cold-start performance is gated by the existing benchmark.
- **Pure-additive at the type level.** Every existing dependent crate compiles unchanged. The new symbols (`MemoryStore` trait, `Runtime::store_handle()`) are additions; nothing is removed or renamed.
- **Soft-delete is the only behavioural change**, and it is gated to the `facts` table only. Default reads skip soft-deleted rows, which matches user expectation; the hard-delete path remains available on `Store` for emergency cleanup.

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Cold-start regression from trait dispatch | Low | The `bench_memory_search_cold_start` benchmark is rewired through `&dyn MemoryStore` so trait dispatch is on the measured path. The verification gate fails the change if p95 exceeds 50 ms. Today's measurement is 1.87–1.94 ms, leaving substantial headroom. |
| Trait surface churn after first external bind | Medium | Only one in-tree implementor (`Store`) exists at the time this change lands. The trait surface is revisitable as a non-breaking expansion. Any breaking shape change after the sister repo binds will be coordinated through a paired follow-up change. |
| Soft-delete migration lock contention | Low | The migration adds a nullable column and a partial index. SQLite handles `ALTER TABLE ADD COLUMN` cheaply for nullable columns without rewriting rows. Index build is on an empty filtered set at migration time. |
| Default-hide read filter missed in some code path | Medium | Verification includes a grep audit of every `FROM facts` SQL string in `crates/kernex-memory/src/store/`. The task list enumerates the six known read sites that must filter `WHERE deleted_at IS NULL`. |
| Trait method count grows beyond the 14 listed | Low | The 14 methods enumerated above are exactly the public surface that downstream consumers reach today. Pre-execution audit (Step 0) confirms that the 14 enumerate cleanly from current `Store` source. Future additions are non-breaking. |

## Cross-repo coordination

Single-repo `kernex-dev/kernex` only. No paired PR.

**Depends on:**
- `workspace-profile-baseline` (already archived in `openspec/archive/`)
- `workspace-crate-split` (already archived in `openspec/archive/2026-05-workspace-crate-split/`)

**Enables (in a sister-repo follow-up change):**
- The sister repo `kernex-dev/kernex-agent` will bind its `memory-cli` cfg slot (reserved by the previously landed `cargo-feature-graph` change) against the new `MemoryStore` trait. That binding is a sister-repo follow-up change and is not part of this proposal.

**Sequencing:** this change ships independently. The sister-repo follow-up change is unblocked once a published version of `kernex-runtime` re-exports the trait, but ships on its own timeline.
