# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.3] - 2026-05-19

Additive `kernex-adapter-core` release introducing a project-local write surface so adapters that target files outside the home-rooted `config_root` (Codex `<cwd>/AGENTS.md`, Cursor `.cursorrules`, etc.) can declare a separate project root without bypassing the adapter sandbox. Wire format remains back-compatible with 0.8.2 via `#[serde(default)]`. Tag: [v0.8.3](https://github.com/kernex-dev/kernex/releases/tag/v0.8.3).

### Added

- **`kernex-adapter-core`**: new `Detection::project_root: Option<PathBuf>` field on the `#[non_exhaustive]` `Detection` struct. Additive; `#[serde(default)]` keeps the wire format back-compatible with 0.8.2 callers.
- **`kernex-adapter-core`**: new `Detection::with_project_root(installed, config_root, project_root, version)` constructor for adapters that write project-local files. `Detection::new(installed, config_root, version)` is retained source-compatibly and sets `project_root: None`.
- **`kernex-adapter-core`**: `detection_with_project_root_roundtrips` and `detection_deserializes_legacy_wire_shape_without_project_root` smoke tests pin the new field and the back-compat path.

### CI / Release tooling

- Workspace release cut by release-plz with the per-crate shared-tag fix from 0.8.1 (only the `kernex` umbrella creates the `v{version}` tag; 8 other publishable members keep `git_tag_enable = false`).

## [0.8.2] - 2026-05-19

Additive `kernex-adapter-core` release. Adds a plain public constructor so downstream consumers can build `Detection` without routing through `serde_json::from_value` while the struct remains `#[non_exhaustive]`. Tag: [v0.8.2](https://github.com/kernex-dev/kernex/releases/tag/v0.8.2).

### Added

- **`kernex-adapter-core`**: `Detection::new(installed, config_root, version)` public constructor (FU-E-01). Removes the `serde_json::from_value(json!({...}))` workaround pattern that `kernex-agent`'s adapter `detect` paths had to use against the `#[non_exhaustive]` struct. Wire format unchanged; pinned by the new `detection_new_roundtrips` smoke test.

### CI / Release tooling

- **`actions/create-github-app-token`** bumped 3.1.1 -> 3.2.0 in `.github/workflows/release-plz.yml` and `.github/workflows/publish-crates.yml`.
- **`release-plz/action`** bumped to the latest minor.

## [0.8.1] - 2026-05-14

Process-only release. No public API change. Tag: [v0.8.1](https://github.com/kernex-dev/kernex/releases/tag/v0.8.1).

### CI / Release tooling

- **release-plz shared-tag 422 fix**: `git_tag_enable = false` set on 8 of the 9 publishable packages in `release-plz.toml`, leaving only the `kernex` umbrella crate as the single tag source for the workspace's shared `v{version}` tag. Resolves the 422 `Reference already exists` loop observed on the v0.8.0 publish, where the first crate created the tag and the next 8 received a duplicate-ref error (the publish chain still fired on the tag push, but the release-plz workflow itself exited 1). The 7 leaf crates plus `kernex-adapter-core` still bump and publish to crates.io; they just no longer try to create the shared tag.
- **`actions/checkout`** bumped 4.3.1 -> 6.0.2.

### Documentation

- Internal-planning identifier leaks (FU-B-02, FU-A-03, planning-doc cross-refs, sprint tags, slice names, and a stray internal phase + step identifier in the `kernex-memory` `observations.rs` module doc) scrubbed from source comments, the workspace and `kernex-memory` CHANGELOGs, one workflow file, and `Cargo.toml`. Runtime behavior unaffected; the .crate tarballs already on crates.io for the 0.6.x / 0.7.x / 0.8.0 release lines still ship the original wording, but GitHub readers from this release forward see the scrubbed text.

## [0.8.0] - 2026-05-12

Breaking `kernex-memory` release. Introduces a typed observation log alongside the existing conversation, fact, and scheduled-task surfaces, and renormalizes `MemoryStore::get_memory_stats` to a 4-tuple to surface the new dimension. Tag: [v0.8.0](https://github.com/kernex-dev/kernex/releases/tag/v0.8.0).

### Added

- **`kernex-memory`**: migration `018_observations.sql` creates the `observations` base table plus an FTS5 mirror with three sync triggers. UUIDv4 ids, soft-delete via `deleted_at`, `sender_id` is the intra-DB scope (the on-disk DB is the project scope).
- **`kernex-memory`**: five new `MemoryStore` trait methods: `save_observation`, `get_observation_by_id`, `search_observations`, `soft_delete_observation`, `list_soft_deleted_observations`. Soft-deleted rows are invisible to `get` / `search` / `stats` and visible only via `list_soft_deleted_observations`.
- **`kernex-memory`**: public `ObservationType` enum (7 variants, snake_case serde, `#[non_exhaustive]`), `SaveEntry` input shape (optional structured fields `what` / `why` / `where_field` / `learned`), and `Observation` output row (`id` plus timestamps). All three types are `#[non_exhaustive]` for forward-compat.

### BREAKING

- **`kernex-memory`**: `MemoryStore::get_memory_stats` now returns `Result<(i64, i64, i64, i64), MemoryError>` (was a 3-tuple). New shape is `(conversations, messages, observations, facts)`; `observations` slots in at position 2. Downstream consumers must destructure four elements. Bundled with the trait additions so one breaking release covers the full migration.

### Tests

- `kernex-memory`: +18 in-tree store tests + 3 type unit tests (21 new). Existing `test_memory_stats_excludes_soft_deleted_facts` updated for the 4-tuple. `kernex-runtime/examples/full_stack.rs` updated likewise.

## [0.7.0] - 2026-05-11

Breaking `kernex-memory` release. Replaces the untyped tuple return shapes on `MemoryStore::search_messages` and `MemoryStore::get_history` with typed row structs, and pushes recency filtering server-side via a new `since` parameter on `search_messages`. Tag: [v0.7.0](https://github.com/kernex-dev/kernex/releases/tag/v0.7.0).

### Added

- **`kernex-memory`**: public `MessageRow { id, conversation_id, role, content, timestamp: SystemTime }` and `HistoryRow { conversation_id, summary, updated_at: SystemTime }` in `crates/kernex-memory/src/types.rs`. Timestamps parse from the SQLite `TIMESTAMP` column at fetch time via `chrono::NaiveDateTime` -> `DateTime<Utc>` -> `SystemTime`; consumers never see raw timestamp strings.
- **`kernex-memory`**: `MemoryStore::get_message_by_id(id: &str) -> Result<Option<MessageRow>>` trait method. Unblocks the downstream `kx mem get <id>` wire that had been returning `CliError::NotImplemented`.
- **`kernex-memory`**: `since: Option<SystemTime>` parameter on `MemoryStore::search_messages` pushes recency filtering server-side via `WHERE m.timestamp >= ?` so `LIMIT` applies post-filter (resolves the recency-cutoff ambiguity flagged in the typed-row design notes).

### BREAKING

- **`kernex-memory`**: `MemoryStore::search_messages` return type `Vec<(String, String, String)>` -> `Vec<MessageRow>`. SQL now selects `id` + `conversation_id` alongside `role` / `content` / `timestamp`.
- **`kernex-memory`**: `MemoryStore::get_history` return type `Vec<(String, String)>` -> `Vec<HistoryRow>`. SQL now selects `conversations.id` alongside the existing `summary` + `updated_at`.

### Documentation

- Repo docs aligned with the 0.6.2 ship state ahead of this cut.

## [0.6.2] - 2026-05-11

First end-to-end release-plz cycle. Release PR #17 squash-merged at `87b7fa6` triggered the publish chain via the GitHub App identity wired in v0.6.1; all 9 publishable crates shipped automatically.

### Performance

- **`kernex-memory`**: migrations fast-path in `Store::run_migrations`. The per-migration `SELECT name FROM _migrations WHERE name = ?` round-trip loop (17 migrations at time of writing) is replaced by one `SELECT name FROM _migrations` plus an in-memory `HashSet<String>` membership check. Drops the cold-open cost from O(N) network round-trips to O(1).

### Changed

- **Workspace**: `toml = "0.8"` swapped for `basic-toml = "0.1"` (dtolnay's minimal serde-compatible TOML parser) across `kernex-core`, `kernex-pipelines`, `kernex-presets`, `kernex-skills`. `toml_edit` is now fully absent from the workspace dep graph. Per-feature gating documented in the FU-B-01 research note is no longer needed because `basic-toml` is a drop-in replacement that avoids the architectural restructure.

### CI / Release tooling

- **GitHub App identity for release-plz** (`actions/create-github-app-token@v3.1.1`) replaces the workflow's default `GITHUB_TOKEN`. The App (owned by `kernex-dev`, installed on `kernex-dev/kernex` with Contents + Pull requests read+write) mints a short-lived installation token at job time. Because GitHub Actions blocks recursive workflow triggers for refs pushed by `GITHUB_TOKEN`, this is the only way release-plz's Release PR branch can fire CI on the PR, and its tag push can auto-fire `publish-crates.yml`. Without this, every cycle required a manual `git push --delete origin v* && git push origin v*` from a developer machine to fire the publish chain.
- **`workflow_dispatch` trigger** added to `publish-crates.yml` as an escape hatch (`gh workflow run publish-crates.yml --ref v<version>`). Used for emergency republishes, yank recovery, or when the App token mint fails.
- **release-plz workspace-level `publish = false`** added to `release-plz.toml` so release-plz only tags and never tries to `cargo publish`. Publishing is delegated entirely to `publish-crates.yml` (which uses OIDC).
- **release-plz workflow defaults** restored: dropped the `command: release-pr` override so the action runs both `release-pr` (open Release PR) and `release` (create tag on Cargo.toml > last-tag drift) in sequence.
- **Cold-start CI gate** promoted from informational to hard-fail at p95 ≤ 50 ms. New `cold_start_gate` job in `.github/workflows/ci.yml` runs `cargo bench -p kernex-bench --bench cold_start -- memory_search_cold_start` and parses the criterion `time:` line via `scripts/check-cold-start.sh`.

## [0.6.1] - 2026-05-10

First release through the new OIDC trusted-publishing pipeline. All 9 crates published cleanly with no long-lived `CARGO_REGISTRY_TOKEN` secret involved; the token was deleted from repo secrets the same day.

### Changed

- **`kernex-memory`, `kernex-runtime`, `kernex` umbrella**: replaced em-dash separators in `Cargo.toml` description fields with periods or colons to align with the project's no-em-dash-in-user-facing-copy rule. Metadata-only; visible on crates.io as of this release.

### CI / Release tooling

- **Trusted publishing (OIDC)** wired into `.github/workflows/publish-crates.yml` via `rust-lang/crates-io-auth-action@v1.0.4`. Each of the 9 publishable crates is registered as a trusted publisher at `https://crates.io/crates/<name>/settings -> Trusted Publishing` with `owner=kernex-dev / repo=kernex / workflow=publish-crates.yml / environment=release`. Publish job is bound to the `release` GitHub Environment for optional protection rules.
- **`scripts/validate-publish-chain.sh`** plus matching CI job catches publishable crates that depend on `publish = false` workspace members via path. Designed to surface the class of failure that broke the v0.6.0 cut mid-chain.
- **`Swatinem/rust-cache`** pinned to `prefix-key: v1-rust-2026-05-10` across `ci.yml` and `publish-crates.yml` to invalidate a stale cache namespace that was poisoning `Test (ubuntu-22.04)` runs.

## [0.6.0] - 2026-05-10

All 9 publishable crates shipped to crates.io on 2026-05-10. See the GitHub Release at [v0.6.0](https://github.com/kernex-dev/kernex/releases/tag/v0.6.0) for full release notes. The publish chain hit two issues mid-flight (revoked pre-2020 token, then `publish = false` blast radius on `kernex-runtime`); the second forced an in-cycle promotion of `kernex-adapter-core` to public.

### Added

- New public crate `kernex-adapter-core` defining the `Adapter` trait, `AdapterId` enum, `Capability` flags, `Detection` outcome, `AdapterError`, `AdapterRegistry`, and a `new_adapter` factory. Originally introduced as `publish = false`; promoted to `publish = true` at commit `586509e` during the release cut to unblock `cargo publish -p kernex-runtime`.
- New workspace-internal crate `kernex-presets` shipping a TOML preset loader plus five empty preset stubs (`full-kernex`, `security-hardened`, `airgapped-defense`, `solo-dev`, `ci-only`). Loader returns `PresetError::Empty` for stub bodies. `publish = false`.
- New workspace-internal crate `kernex-brain` shipping a `BrainStore` trait scaffold with stub method bodies. Trait surface only; implementations land in a follow-up change. `publish = false`.
- `kernex-runtime` now re-exports `Adapter`, `AdapterId`, `AdapterError`, `AdapterRegistry`, and `Capability` from `kernex-adapter-core`, so downstream consumers reach the adapter trait surface through a single import path.
- **kernex-memory**: new public `MemoryStore` trait covering the 17-method conversation, fact, and scheduled-task surface that downstream consumers (runtime composition, future CLI/HTTP/MCP frontends) call today. The trait is `Send + Sync`, object-safe, and uses `#[async_trait]`. `kernex-memory` re-exports `MemoryStore` and the `into_handle` constructor for use by integrators.
- **kernex-memory**: soft-delete on `facts` via a new `deleted_at` column. Adds `Store::soft_delete_fact`, `Store::soft_delete_facts`, and `Store::list_soft_deleted_facts` (recovery / debug helper). Default-filtered reads (`get_fact`, `get_facts`, `get_all_facts`, `get_all_facts_by_key`, `is_new_user`, `find_canonical_user`, `get_memory_stats`) skip soft-deleted rows. Migration `017_soft_delete.sql` adds the column and a partial index `idx_facts_active (sender_id, key) WHERE deleted_at IS NULL`.
- **kernex-runtime**: `Runtime::store_handle()` returns `Arc<dyn kernex_memory::MemoryStore>` so a binary consumer can share the runtime's composed `Store` instance instead of opening a second SQLite connection against the same database file (gated on the `sqlite-store` feature).
- **Release tooling**: release-plz adopted as the version-bump + CHANGELOG-update + tag-creation engine for the workspace. Per-crate `CHANGELOG.md` files now auto-generated under `crates/*/CHANGELOG.md`, supplementing this workspace-level CHANGELOG. `.github/workflows/release-plz.yml` opens a draft Release PR on every push to main; `.github/workflows/publish-crates.yml` runs the pre-publish gate and ships the 9-crate dependency-ordered chain on `v[0-9]+.[0-9]+.[0-9]+` tag pushes.

### Changed

- Workspace version bumped from `0.5.0` to `0.6.0` (additive re-exports in `kernex-runtime`; no removed or renamed symbols).
- The seven existing publishable library crates and the `kernex` umbrella follow the workspace version bump.
- **kernex-memory**: `Store::store_fact` now clears `deleted_at` on upsert, so re-storing a previously soft-deleted key restores the row to default-filtered reads. The hard-delete methods (`Store::delete_fact`, `Store::delete_facts`) remain inherent-only and are deliberately not exposed on the `MemoryStore` trait, keeping the default consumer path on the recoverable soft-delete API.
- **bench/cold_start**: `memory_search_cold_start` now dispatches through `Arc<dyn MemoryStore>::search_messages` so the cold-start benchmark validates the trait surface that downstream consumers call into, not a bypassed direct-struct path.

## [0.5.0] - 2026-05-07

### BREAKING

This release closes audit item **M5** (per-crate `thiserror` enums replacing the stringified `KernexError::Variant(String)` shape) and re-architects error types so callers can pattern-match on the typed cause via `Error::source()` chain inspection or `Box::downcast_ref::<T>()`.

#### KernexError variant shape changed (kernex-core 0.5.0)

The cross-crate variants `Provider`, `Store`, `Sandbox`, `Pipeline`, and `Skill` now wrap a `Box<dyn std::error::Error + Send + Sync + 'static>` instead of a `String`. Callers that pattern-matched on the inner string must update:

```rust
// before
match err {
    KernexError::Provider(s) if s.contains("timeout") => /* retry */,
    _ => /* other */,
}

// after
match err {
    KernexError::Provider(boxed) => {
        if let Some(p) = boxed.downcast_ref::<kernex_providers::ProviderError>() {
            match p {
                ProviderError::Http { source, .. } if source.is_timeout() => /* retry */,
                ProviderError::Serde { .. } => /* parse error */,
                _ => /* other */,
            }
        }
    }
    _ => /* other */,
}
```

`Config` and `Guardrail` variants are unchanged (still `String`); they carry no foreign source to preserve.

The new `KernexError` exposes typed constructors (`KernexError::provider(e)`, `::store(e)`, `::sandbox(e)`, `::pipeline(e)`, `::skill(e)`) that accept any `E: Error + Send + Sync + 'static` and box internally.

#### New per-crate error types

- **`kernex-memory`** — `MemoryError` with `Sqlite { context, source }`, `Io { context, source }`, `Serde { context, source }`, `Logic(String)` variants. `From<MemoryError>` for `KernexError` boxes into `KernexError::Store`.
- **`kernex-providers`** — `ProviderError` with `Http { context, source }`, `Serde { context, source }`, `Io { context, source }`, `Config(String)`, `Logic(String)` variants. `Config` hoists to `KernexError::Config`; everything else boxes into `KernexError::Provider`.
- **`kernex-pipelines`** — `PipelineError` with `Toml { context, source }`, `Io { context, source }`, `Logic(String)` variants. Boxes into `KernexError::Pipeline`.
- **`kernex-skills`** — `SkillError` with `Io { context, source }`, `Logic(String)` variants. Boxes into `KernexError::Skill`.

Each per-crate enum implements `std::error::Error` via `thiserror`; `Send + Sync + 'static` bounds are satisfied automatically. The `#[source]` attribute on struct variants preserves the cause chain so `error.source()` walks to the underlying `sqlx::Error`/`reqwest::Error`/etc.

#### Why this design (citations)

- **Rust API Guidelines C-GOOD-ERR** (https://rust-lang.github.io/api-guidelines/interoperability.html): "Error types should always implement the std::error::Error trait... error types should implement the Send and Sync traits."
- **BurntSushi, *Rust Error Handling*** (https://burntsushi.net/rust-error-handling/): libraries should "define your own error type and implement the std::error::Error trait" with structured enum variants, not opaque strings.
- **`tower::Service` precedent** (https://docs.rs/tower/latest/tower/trait.Service.html): for traits used via `dyn Trait` where multiple impls have different error types, the canonical pattern is per-impl typed errors with `Box<dyn Error + Send + Sync>` boxing at the dispatch boundary.

The aggregator stays in `kernex-core` (rather than moving to `kernex-runtime`) because the boxed-trait-object pattern resolves the dependency-cycle concern without restructuring: each per-crate crate provides its own `From<TheirError> for KernexError` impl, so `kernex-core` never has to depend on `kernex-providers`, `kernex-memory`, etc.

### Migration notes for downstream crates

- `kernex-agent` consumes `kernex-runtime` via `anyhow::Result` end-to-end; **no source changes required** (the agent does not pattern-match on `KernexError` variants).
- Direct downstream users that match on `KernexError::Provider(String)` etc. must switch to the downcast pattern shown above.
- The stringified message is still recoverable via `format!("{err}")` for logging — the `#[error(transparent)]` Display delegates through the boxed source, so log output is unchanged.

## [0.4.2] - 2026-05-07

### Security

- **kernex-providers**: bound streaming response buffers (1 MiB Anthropic SSE; 8 MiB MCP `LinesCodec`) so a hostile or runaway server cannot exhaust memory; stop cloning the API key into a local `String` before each request and pass the `SecretString` reference straight into the `Authorization` / `x-api-key` header.
- **kernex-providers / kernex-skills**: drop dynamic-linker env vars (`LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`) from skill-supplied environment maps before spawning MCP servers and toolboxes; closes the sandbox-escape vector flagged as N13.
- **kernex-memory**: pre-create the SQLite file at mode `0o600` *before* sqlx connects, closing the TOCTOU window where `sqlx::sqlite::SqliteConnectOptions::create_if_missing(true)` briefly created a world-readable file (N6).
- **kernex-core**: nested-arg-aware permission checks — `flatten_strings` now recurses into JSON arrays/objects so payloads cannot smuggle dangerous arguments through nesting; `MAX_ARGS_LEN = 64 KiB` cap on the flattened representation.
- **kernex-providers**: `validate_gemini_model_id` enforces `[A-Za-z0-9._-]{1,128}`; `claude_code/command.rs::looks_like_cli_flag` filters values starting with `-` from `model`, `session_id`, and `agent_name` so context-poisoned strings cannot inject CLI flags into the spawned `claude` invocation. `parse_retry_after` honors the `Retry-After` header (clamped to 30 s, max with exponential backoff).
- **kernex-providers**: MCP `protocolVersion` pinned to `"2025-03-26"` — a published spec date — so MCP servers cannot probe for unreleased protocol behaviors via wildcard or future-dated handshakes (N25).

### Added

- **kernex-core**: MSRV polyfill `utf8::floor_char_boundary()` so the workspace continues to support Rust 1.74 even though `str::floor_char_boundary` only stabilizes in 1.83. Used at four sanitization sites.
- **kernex-memory**: prompt-cache token breakdown persisted (migration `016_cache_token_breakdown.sql` adds `cache_read_input_tokens`, `cache_creation_input_tokens`, `prompt_tokens`, and `completion_tokens` to `token_usage`); `Store::record_usage_full(UsageBreakdown)` and `Store::get_total_usage()` round-trip the new columns.
- **kernex-providers**: `cache_read_input_tokens` and `cache_creation_input_tokens` exposed on `AnthropicUsage` and surfaced in `CompletionMeta` so callers can attribute cache hits.

### Changed

- **kernex-runtime**: `warn_if_data_dir_unusual()` emits a `tracing::warn!` when `KERNEX_DATA_DIR` resolves outside `$HOME` / `/tmp` / `/var`, catching the common typo class where data ends up in `/`.
- **kernex-memory**: `Store::consolidator::spawn_cancellable` accepts an `Option<watch::Receiver<bool>>` for cooperative shutdown; `SystemTime::duration_since` no longer silently swallows clock-jump errors via `unwrap_or_default()`.
- **kernex-runtime**: `Runtime::complete*` now constructs a `UsageBreakdown` and routes through `record_usage_full`, replacing the truncating two-column write path.

## [0.4.1] - 2026-05-07

### Added

- **kernex-runtime**: `RuntimeBuilder::auto_compact(bool)` opt-in. When enabled, the runtime reuses the active provider as a `Summarizer` (one extra round-trip per overflow event, not per turn) and prepends `[Earlier conversation summary]` to the system prompt instead of silently dropping the oldest messages. Default off for v0.4.x backward compatibility.
- **kernex-core**: `ContextNeeds` now derives `Debug, Clone` so callers can override individual fields without rebuilding the struct.
- **kernex (umbrella crate)**: re-exports `kernex_runtime::*` so `cargo add kernex` works.

### Changed

- **kernex-memory**: `Store::build_context` now emits `tracing::warn!` when history overflows `max_context_messages` and the Drop strategy is in effect (no summarizer wired in). One log per overflow event with conversation id and overflow count, pointing operators at `RuntimeBuilder::auto_compact`.

### Security

- **kernex-providers**: bumped `rustls-webpki` to 0.103.13 via lock file (RUSTSEC-2026-0098 / 0099 name-constraint bypasses, RUSTSEC-2026-0104 reachable panic in CRL parsing). Bumped `rand` to 0.8.6 / 0.9.4 (RUSTSEC-2026-0097 unsoundness warning).
- **workspace**: `serde_yml` (RUSTSEC-2025-0068, archived crate, unsound) replaced by `serde_yaml_ng`.
- **kernex-memory**: `memory.db` and parent dir now chmod 0600 / 0700 on Unix.
- **kernex-providers**: web fetch tool rejects loopback / RFC1918 / link-local / CGNAT / multicast / metadata IPs (IPv4 and IPv6); `redirect::Policy::none()` so attacker-controlled redirects can't bypass the check. Grep tool caps regex pattern length and passes `--regex-size-limit 16M` to ripgrep.
- **kernex-pipelines**: agent and topology names validated as path segments (no `..`, no separators, no high-bit characters); canonical-prefix symlink guard.
- **kernex-sandbox**: Seatbelt SBPL injection blocked at every interpolation site; new `SandboxProfile::require_os_enforcement` flag returns `ErrorKind::Unsupported` on hosts lacking Seatbelt/Landlock; Landlock `pre_exec` is now allocator-safe (build_ruleset in parent, child only locks Mutex and calls restrict_self).
- **kernex-providers**: error bodies truncated at 16 KB; HTTPS required for keyed providers.
- **kernex-skills**: `Permissions::allows_command` enforced for MCP servers and toolboxes.
- **CI**: `cargo deny check` runs the full advisories + bans + licenses + sources suite. New `deny.toml` rejects openssl / native-tls (rustls-only policy) and pins the license allow-list.

### Added (continued)

- **kernex-providers**: Groq, Mistral, DeepSeek, Fireworks, and xAI as named provider strings in `ProviderFactory::create()` — each resolves to the OpenAI provider with the correct `base_url` and default model. Provider count: 11 built-in (+ Bedrock feature-gated).
- **kernex-providers**: AWS Bedrock provider with SigV4 request signing — supports Anthropic Claude models on Bedrock; opt-in via the `bedrock` Cargo feature.
- **kernex-runtime**: `Runtime::complete_stream()` and `complete_stream_with_needs()` — surfaces streaming from `StreamingProvider` implementations through the public `Runtime` API, returning a `tokio::sync::mpsc::Receiver<StreamEvent>`.
- **kernex-runtime**: `RuntimeBuilder::from_file(path)` — load a declarative TOML or YAML agent definition; maps `[runtime]` and `[memory]` sections into the builder without Rust code.
- **kernex-runtime**: `opentelemetry` optional Cargo feature — enables `tracing-opentelemetry` and `opentelemetry` crates for export to Jaeger, Honeycomb, Grafana, and any OTel-compatible backend.
- **kernex-core**: `GuardrailRunner` trait with `check_input` / `check_output` lifecycle hooks and `GuardrailAction::Allow`, `Block(String)`, `Sanitize(String)` outcomes; injectable via `RuntimeBuilder::with_guardrails()`.
- **kernex-memory**: `PhaseCheckpoint` — pipeline run checkpointing to SQLite; `Store::upsert_phase_checkpoint()` and `Store::get_run_checkpoints()` enable crash-safe phase state tracking (pending, in_progress, completed, failed).
- **kernex-core**: `MemoryConfig::max_connections` — configurable SQLite connection pool size (default: 4).
- **kernex-providers**: `timeout_secs` field on all five HTTP provider TOML configs (`AnthropicConfig`, `OpenAiConfig`, `OllamaConfig`, `OpenRouterConfig`, `GeminiConfig`) and `ProviderConfig::timeout_secs` in `ProviderFactory` (default: 120 s); all HTTP providers expose a `with_timeout(secs) -> Self` builder.
- **kernex-pipelines**: `Phase::parallel_group` — optional named group field; consecutive phases sharing a name are collected by `LoadedTopology::phase_groups()` into `PhaseGroup` structs for concurrent execution. Single-phase groups remain sequential.
- **kernex-runtime**: `full_stack` example — end-to-end demo using `MockProvider`, memory, skills, and a 2-phase pipeline with corrective loop; no API key required.

### Changed

- **docs**: Landlock module doc updated with full ABI version table (V1/5.13 through V5/6.12), WSL1 and container edge cases, and partial-enforcement warning behavior.
- **kernex-sandbox** (`lib.rs`): Linux Landlock description clarified — 5.13 is the minimum for any OS-level enforcement; 6.12 gives full ABI::V5 coverage; older kernels apply best-effort protection.

## [0.4.0] - 2026-03-09

### Added

- **kernex-runtime**: `Runtime::run()` with `RunConfig` and `RunOutcome` — wraps provider completions with max-turns enforcement and fires the `on_stop` hook after each turn
- **kernex-core**: `HookRunner` trait with `pre_tool` / `post_tool` / `on_stop` lifecycle methods and `HookOutcome::Allow` / `Blocked(String)` variants
- **kernex-core**: `NoopHookRunner` default implementation (no-op, zero overhead)
- **kernex-providers**: Prompt caching support for Anthropic — split system prompt at `CACHE_BOUNDARY` marker into a cacheable stable prefix and a dynamic suffix; adds `anthropic-beta: prompt-caching-2024-07-31` header automatically when caching is active
- **kernex-skills**: 12 builtin agent skills in `examples/skills/builtin/` — Core: `frontend-developer`, `backend-architect`, `security-engineer`, `devops-automator`, `reality-checker`, `api-tester`, `performance-benchmarker`. Specialist: `senior-developer`, `ai-engineer`, `accessibility-auditor`, `agents-orchestrator`, `project-manager`
- **docs**: `docs/kernex-agent.md` — implementation spec for the `kx` CLI binary with provider resolution, runtime wiring, hook runner scaffold, and KAIROS scheduler reference

## [0.3.2] - 2026-03-07

### Fixed

- **kernex-memory**: Added missing `macros` feature to tokio dependency (fixes `tokio::join!`)
- **kernex-providers**: Added missing `macros` feature to tokio dependency (fixes `tokio::select!`)

## [0.3.1] - 2026-03-07

### Added

- **docs**: CONTRIBUTING.md with development setup and PR process
- **docs**: ARCHITECTURE.md expanded with custom Provider guide and kernex-agent reference

### Changed

- **crates.io**: Added documentation URL, keywords, and categories for discoverability
- **clippy**: Enforced `deny(clippy::unwrap_used, clippy::expect_used)` across all crates

## [0.3.0] - 2026-03-07

### Added

- **kernex-providers**: SandboxProfile integration for AI Providers and ProviderFactory
- **kernex-runtime**: `Runtime::complete()` API for simplified completions
- **fuzz**: cargo-fuzz support for `truncate_output`

### Changed

- Anonymized codebase by removing original project name references

## [0.2.0] - 2026-03-06

### Added

- **kernex-core**: `env` field on `McpServer` for passing environment variables to MCP server processes
- **kernex-skills**: `mcp.json` support — load MCP servers from optional JSON file in skill directories, merged with frontmatter (JSON takes precedence on name collision)
- **kernex-skills**: `AGENTS.md` support as modern alternative to `ROLE.md` for project instructions
- **kernex-providers**: Environment variables propagated to MCP subprocess spawn and Claude Code settings
- 4 runnable examples: `simple_chat`, `memory_agent`, `skill_loader`, `pipeline_loader`
- 7 reference skills: filesystem, git, playwright, github, postgres, sqlite, brave-search
- Code-review pipeline topology example with 4 agents
- 298 tests across all crates (+12 from v0.1.0)

## [0.1.0] - 2026-03-06

### Added

- **kernex-core**: Shared types (`Request`, `Response`, `Context`), traits (`Provider`, `Store`), config loading, input sanitization
- **kernex-sandbox**: OS-level sandboxing with Seatbelt (macOS) and Landlock (Linux), code-level path protection
- **kernex-providers**: 6 AI providers (Claude Code CLI, Anthropic, OpenAI, Ollama, OpenRouter, Gemini), tool executor with sandbox enforcement, MCP client over stdio
- **kernex-memory**: SQLite-backed persistent memory with 13 migrations, FTS5 full-text search, conversation lifecycle, user facts, scheduled tasks with dedup, reward-based learning (outcomes/lessons), project-scoped sessions
- **kernex-skills**: Skill loader compatible with Skills.sh standard (`SKILL.md` + TOML/YAML frontmatter), project loader (`ROLE.md`), trigger matching, MCP server activation, flat-to-directory migration
- **kernex-pipelines**: TOML-defined topology format for multi-agent pipelines, phase types (standard, parse-brief, corrective-loop, parse-summary), model tier selection, pre/post validation, agent .md loading
- **kernex-runtime**: Facade crate with `RuntimeBuilder` for composing all subsystems
- Dual license: Apache-2.0 OR MIT
- 286 tests across all crates
