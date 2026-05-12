# Tasks: Typed observation table

> Execution checklist for the `typed-observation-table` change.

## 1. Migration

- [x] Add `crates/kernex-memory/migrations/018_observations.sql` with base table + FTS5 mirror + 3 triggers (insert / update / delete sync).
- [x] Register the migration in `Store::run_migrations` (under the existing fast-path registry).

## 2. Rust types

- [x] Add `crates/kernex-memory/src/observation.rs` with `ObservationType`, `SaveEntry`, `Observation`. All three `#[non_exhaustive]`.
- [x] Wire `pub mod observation;` and `pub use observation::{Observation, ObservationType, SaveEntry};` in `crates/kernex-memory/src/lib.rs`.
- [x] Add `serde` to `kernex-memory`'s `[dependencies]` (with `features = ["derive"]`); previously only `serde_json` was direct.
- [x] Write 3 unit tests in `observation.rs::tests`: round-trip via DB strings, unknown DB string returns None, JSON serde round-trip.

## 3. `Store` inherent methods

In `crates/kernex-memory/src/store/observations.rs` (new file):

- [x] `Store::save_observation` — UUIDv4 id, `created_at == updated_at == now`. Maps `sqlx::Error` to `MemoryError::sqlite("insert observation failed", source)`.
- [x] `Store::get_observation_by_id` — SELECT WHERE `id = ? AND deleted_at IS NULL`. Soft-deleted ids return `Ok(None)`.
- [x] `Store::search_observations` — dynamic SQL with optional `created_at >=` and optional `type =` branches. Result order: FTS5 `rank` ascending then `created_at` descending tiebreaker.
- [x] `Store::soft_delete_observation` — UPDATE with `deleted_at = now` guarded by `AND deleted_at IS NULL`. Returns `Ok(rows_affected > 0)`.
- [x] `Store::list_soft_deleted_observations` — SELECT WHERE `sender_id = ? AND deleted_at IS NOT NULL ORDER BY deleted_at DESC`.
- [x] `Store::count_observations` (private helper) — used by `get_memory_stats`.

## 4. Trait surface update

In `crates/kernex-memory/src/memory_store.rs`:

- [x] Add the 5 new method signatures to the `MemoryStore` trait with rustdoc.
- [x] Add the 5 forwarding impls inside `impl MemoryStore for Store`.
- [x] **Breaking change:** update `get_memory_stats` signature from `(i64, i64, i64)` to `(i64, i64, i64, i64)`. Update trait + impl + inherent `Store::get_memory_stats` to slot in the observation count.

## 5. Tests

In `crates/kernex-memory/src/store/tests.rs`:

- [x] `save_round_trip`
- [x] `save_then_search_finds`
- [x] `save_then_get_by_id`
- [x] `save_with_none_optionals`
- [x] `save_rejects_empty_title`
- [x] `save_rejects_unknown_type_at_db`
- [x] `search_kind_filter_narrows`
- [x] `search_since_filters_by_recency`
- [x] `search_sender_scope_is_hard`
- [x] `search_empty_corpus_returns_empty_vec`
- [x] `soft_delete_hides_from_default_reads`
- [x] `soft_delete_is_idempotent`
- [x] `soft_delete_missing_id_returns_false`
- [x] `list_soft_deleted_returns_only_deleted`
- [x] `list_soft_deleted_respects_sender_scope`
- [x] `get_memory_stats_returns_four_tuple`
- [x] `get_memory_stats_excludes_soft_deleted_observations`
- [x] `migration_018_applies_idempotently`

Test count delta in `kernex-memory`: +18 (lib) + 3 (observation.rs tests) = +21 total.

## 6. Workspace consumer updates

- [x] Update `crates/kernex-runtime/examples/full_stack.rs` to destructure the new 4-tuple from `get_memory_stats`.

## 7. Workspace release

- [ ] release-plz detects the `feat(memory)!:` conventional commit and opens a v0.8.0 Release PR that bumps every publishable crate from 0.7.0 to 0.8.0 and auto-generates the CHANGELOG entries.
- [ ] Squash-merge the Release PR after review.
- [ ] Publish workflow runs and publishes 9 crates to crates.io.

## 8. Definition of done

- [x] `cargo build --workspace` clean.
- [x] `cargo test --workspace` green; 18 new observation tests + 3 type unit tests pass.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] `cargo fmt --check` clean.
- [ ] `kernex-memory 0.8.0` resolvable via `cargo search kernex-memory`.
- [ ] Downstream consumer (`kernex-agent` Step 2.11) can compile against `kernex-memory = "0.8.0"`.
