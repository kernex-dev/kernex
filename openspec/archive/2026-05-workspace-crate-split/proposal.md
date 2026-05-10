# Proposal: Workspace crate split for adapter, preset, and brain surfaces

> **Change ID:** `workspace-crate-split`
> **Author:** Jose Hurtado
> **Status:** Archived. Landed at `kernex-dev/kernex@53b5537` on 2026-05-10.
> **Estimated effort:** ~5 to 7 working days
> **Repo:** `kernex-dev/kernex` (this repo)
>
> ## Post-merge notes
>
> Three drifts from the proposal as authored:
>
> 1. **Smoke tests rewired off struct-literal construction.** `Preset` and the kernex-brain value types (`HealthScore`, `ConflictRelation`, `DecayRanking`) are `#[non_exhaustive]`, which blocks struct-literal construction outside the defining crate. Smoke tests now exercise serde round-trip against a known JSON body and lean on `::new` constructors. Coverage equivalent.
> 2. **`BrainStore::search` signature tightened.** Originally returned `Vec<i64>`, which leaks the storage representation into the trait API. A `pub struct ObservationId(pub i64)` newtype was introduced and the trait method now returns `Vec<ObservationId>`. `record` and the value types reference `ObservationId` instead of raw `i64` for consistency.
> 3. **`#[non_exhaustive]` re-applied to brain value types.** Required `#[non_exhaustive]` was dropped during initial scaffold to satisfy struct-literal smoke tests, then re-added with constructors after a code review caught the forward-compat regression.

## Operator friction

The runtime workspace at `kernex-dev/kernex` ships seven crates today: `kernex` (umbrella), `kernex-core`, `kernex-runtime`, `kernex-providers`, `kernex-memory`, `kernex-pipelines`, `kernex-skills`, plus `kernex-sandbox`. The `workspace-profile-baseline` change installed the size discipline at the library level, and the sister-repo `cargo-feature-graph` change at `kernex-dev/kernex-agent` reserved the cfg surface that the binary side will pull on. The feature graph already names adapter slots (`agent-claude`, `agent-codex`, `agent-cursor`, ...) and preset slots (five `preset-*` flags) as placeholders. Both reserve a shape; neither defines the trait surface that an actual adapter or preset must implement.

Today the trait surface lives nowhere:

1. **No `Adapter` trait.** The agent-side feature flags expect a contract every adapter implements (detect, install, capability flags). Without a workspace-level definition, the first concrete adapter implementation has to invent the trait inside its own module and every later adapter rewrites the integration glue.
2. **No `Preset` loader.** The five `preset-*` features are declared empty arrays. The runtime can wire a feature flag to a preset name, but there is no library entry point that loads a TOML body, validates it against a registry of known adapters, or returns a typed value the runtime can act on.
3. **No memory-domain trait surface.** A future memory-domain implementation needs a stable internal trait it can land against. Putting that trait inside an existing crate either inflates the crate or forces a breaking move when the implementation arrives.
4. **Trait surfaces invented under deadline pressure churn.** Defining the trait surface inside the first concrete adapter implementation guarantees rework: the second adapter discovers a missing capability flag, the third one needs a different error variant, and now every consumer rebuilds.
5. **Public sister-repo dependents need a single import path.** Downstream code in `kernex-dev/kernex-agent` (and any future external consumer) should consume one crate (`kernex-runtime`) and get the adapter trait, registry, and IDs through that crate's re-exports, not learn three separate dependency lines.

This change introduces the missing trait surfaces as workspace-internal crates, plus a one-line re-export so `kernex-runtime` consumers see the new symbols transparently. It is the runtime-side companion to the adapter and preset slots already reserved in `cargo-feature-graph`.

## Solution overview

Add three new workspace member crates and bump `kernex-runtime` from the `0.5.x` line to `0.6.0`:

- `crates/kernex-adapter-core/` defines the `Adapter` trait, an `AdapterId` enum naming the six reserved adapter slots, a `Detection` outcome type, a `Capability` enum, an `AdapterError` via `thiserror::Error #[non_exhaustive]`, an `AdapterRegistry`, a `new_adapter` factory, and a `default_registry` constructor. Object-safe trait, `#[async_trait]` only on I/O methods, sync default-method capability flags. `Arc<dyn Adapter>` for clones. Workspace-internal (`publish = false`) for now; revisit when an external implementor emerges.
- `crates/kernex-presets/` defines a TOML loader, the `Preset` value type, and ships five empty TOML stub files matching the preset names already reserved in the feature graph (`full-kernex`, `security-hardened`, `airgapped-defense`, `solo-dev`, `ci-only`). One-way dep on `kernex-adapter-core` so a `Preset` can name `AdapterId`s. Workspace-internal.
- `crates/kernex-brain/` ships a single trait stub (`BrainStore`) and nothing else. Workspace-internal trait surface for memory-domain operations; implementations land in a follow-up change. The crate exists in this change so the trait can be referenced from `kernex-runtime` without a future cross-crate breaking move.
- `kernex-runtime` bumps `0.5.x` to `0.6.0` and adds `pub use kernex_adapter_core::{Adapter, AdapterId, AdapterError, AdapterRegistry, Capability};` so consumers reach the adapter surface through the runtime they already depend on.

This is **pure-additive** in shape. No existing public API changes. No member crate loses a symbol. The only consumer-visible delta is the runtime version bump (semver-minor on a pre-1.0 crate per Cargo convention) and the new re-exports it carries. `kernex-agent` is not modified by this change.

## Scope

### In scope

1. **`crates/kernex-adapter-core/`** new workspace member. Cargo manifest pinning `thiserror` and `async-trait` from `[workspace.dependencies]`. `publish = false`. `src/lib.rs` exposes:
   - `pub trait Adapter: Send + Sync` with `#[async_trait]` on the two I/O methods (`detect`, `install_command`) and sync default-method capability accessors returning `false`. Object-safe.
   - `pub enum AdapterId { Claude, Codex, OpenCode, Cursor, Cline, Windsurf }` with `Display`, `FromStr`, `Serialize`, `Deserialize`.
   - `pub struct Detection { pub installed: bool, pub version: Option<String>, pub config_path: Option<PathBuf> }`.
   - `pub enum Capability { Detect, Install, Config, Invoke }`.
   - `pub enum AdapterError` via `thiserror::Error` and `#[non_exhaustive]` so adding variants is non-breaking.
   - `pub struct AdapterRegistry` holding `HashMap<AdapterId, Arc<dyn Adapter>>` with `lookup`, `register`, and `ids` methods.
   - `pub fn new_adapter(id: AdapterId) -> Result<Arc<dyn Adapter>, AdapterError>` switch-arm factory mirroring the shape of `crates/kernex-providers/src/factory.rs::ProviderFactory::create`.
   - `pub fn default_registry() -> Result<AdapterRegistry, AdapterError>` driven by `const DEFAULT_ADAPTER_IDS: &[AdapterId]`.
   - `Arc<dyn Adapter>` is the canonical handle for clone-cheap, send-safe sharing.
2. **`crates/kernex-presets/`** new workspace member. Cargo manifest pinning `toml`, `serde`, `thiserror` from `[workspace.dependencies]`, plus a path dep on `kernex-adapter-core`. `publish = false`. `src/lib.rs` exposes:
   - `pub struct Preset { pub adapters: Vec<AdapterId>, pub components: Vec<String> }` with `Serialize`, `Deserialize`.
   - `pub enum PresetError` via `thiserror::Error` and `#[non_exhaustive]`.
   - `pub fn load_preset(name: &str) -> Result<Preset, PresetError>`.
   - Five empty TOML stubs at `crates/kernex-presets/presets/{full-kernex,security-hardened,airgapped-defense,solo-dev,ci-only}.toml`. Each contains a one-line header comment naming the preset and nothing else; the loader returns a `Preset { adapters: vec![], components: vec![] }` for an empty stub.
3. **`crates/kernex-brain/`** new workspace member. Cargo manifest pinning `async-trait` and `thiserror` from `[workspace.dependencies]`. `publish = false`. `src/lib.rs` is a scaffold: a crate-level `#![doc = "..."]` describing the crate as "scaffold; implementations land in a follow-up change", a `pub trait BrainStore: Send + Sync` with stub methods sized for future memory-domain operations (record and search method signatures only), and a `pub enum BrainError` via `thiserror::Error` and `#[non_exhaustive]`. No implementation. No state. No semantics beyond the trait surface.
4. **`kernex-runtime` 0.5.x to 0.6.0 bump.** Update `crates/kernex-runtime/Cargo.toml` `version = "0.5.0"` to `version = "0.6.0"`. Add `kernex-adapter-core = { workspace = true }` to its `[dependencies]`. Add `pub use kernex_adapter_core::{Adapter, AdapterId, AdapterError, AdapterRegistry, Capability};` at the top of `crates/kernex-runtime/src/lib.rs` so existing consumers reach the new surface through `kernex_runtime::Adapter`.
5. **Workspace member glob update.** The existing `members = ["crates/*", "bench"]` glob already picks up new directories under `crates/`, so the manifest line itself does not change. Add the three new internal-crate entries to `[workspace.dependencies]` so members can opt in via `{ workspace = true }`. Bump the workspace version from `0.5.0` to `0.6.0` in `[workspace.package]` and update the existing `kernex-runtime`, `kernex` and other internal-crate version pins under `[workspace.dependencies]` to match if the workspace publishes them in lockstep (the workspace already pins all internal crates at `0.5.0`; this change moves the runtime alone to `0.6.0` and leaves the others on the `0.5.x` line).
6. **Dep-graph audit.** Confirm one-way dep flow: `kernex-presets` depends on `kernex-adapter-core`; `kernex-runtime` depends on `kernex-adapter-core`; `kernex-brain` depends on neither; nothing depends on `kernex-runtime` from inside the new crates. No cycles.

### Out of scope

- Concrete adapter implementations for any of the six `AdapterId` variants. The trait is defined; bodies are not. The `new_adapter` factory returns `AdapterError::NotImplemented` for every arm in this change. A follow-up change wires the first concrete adapter (Claude) against the trait.
- Preset TOML bodies. The five stub files exist as empty headers; their content is filled in a follow-up change, after the first adapter implementation lands and the components vocabulary is concrete.
- `BrainStore` implementations. The trait surface is shipped as a forward-compatibility scaffold. No `MemoryStore` impl, no SQLite layer, no ranking, no persistence semantics. Implementations land in a follow-up change.
- Per-provider feature flags. The native provider matrix in `kernex-providers` stays unconditional. The existing `bedrock` feature in `kernex-providers` is unchanged.
- Any change to `kernex-agent`. The runtime version bump is consumer-visible but mechanically additive: a downstream `cargo update -p kernex-runtime` resolves to `0.6.0`, gains the re-exports, and continues to compile. No paired PR.
- Crates.io publishing for any of the three new crates. All three start as `publish = false`. They are workspace-internal until an external consumer emerges; the question of which (if any) get promoted to crates.io is settled in a follow-up change.
- Trait promotion or signature change for `Provider`, `StreamingProvider`, `Summarizer`, `Store` in `kernex-core`. Out of scope.
- Removal or rename of any existing module or symbol. Pure-additive.

### Cross-repo coordination

Single-repo `kernex-dev/kernex` only. No paired PR in `kernex-dev/kernex-agent`. The change depends on `cargo-feature-graph` having landed at `kernex-dev/kernex-agent` (which reserved the adapter and preset cfg surface this change names) and on `workspace-profile-baseline` having landed in this repo (which expanded `[workspace.dependencies]` and shipped the size-gate workflow templates).

After this change merges and `kernex-runtime 0.6.0` is published to crates.io, `kernex-agent`'s next dependency bump pulls the new re-exports for free. That bump is a follow-up change owned by `kernex-agent`; this change does not coordinate it.

## Why this scope

- **Foundation, not feature.** The adapter trait, the preset value type, and the brain trait surface are load-bearing for every later implementation. Defining them inside the first concrete implementation guarantees rework. Defining them now, in their own crates, locks the shape and lets implementations land cleanly.
- **Pure-additive, low risk.** No existing public API changes. No symbol moves. No behaviour change to the runtime path consumers exercise today. The runtime version bump is a semver-minor coordinated through `[workspace.dependencies]`.
- **Decouples minimal `kx` variants from full runtime in subsequent changes.** Once the trait surface lives in `kernex-adapter-core`, a follow-up change can wire the binary side so a minimal `kx` build links the adapter trait without dragging the full runtime composition. That work is out of scope here. Naming the trait surface in its own crate is the prerequisite.
- **Workspace-internal first, public later.** Shipping all three crates as `publish = false` reserves the right to iterate on the trait surface without a crates.io semver commitment. Promotion to public crates is decided later, when an external consumer asks for it.

## Success criteria

The change ships when:

1. The workspace builds with **10 members**: the existing 7 plus `kernex-adapter-core`, `kernex-presets`, `kernex-brain`. `cargo build --workspace` succeeds.
2. Default features unchanged for every existing crate. `cargo tree -p kernex-runtime --depth 1` shows the new `kernex-adapter-core` dep but no other movement; `kernex-providers`, `kernex-memory`, `kernex-pipelines`, `kernex-skills`, `kernex-sandbox`, `kernex-core` deps are byte-identical to pre-change.
3. `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. `cargo test --workspace` green. The three new crates each ship at least one trivial smoke test (factory error path; loader empty-stub path; trait object construction). No regression in any existing crate's test suite.
5. `cargo fmt --check` clean.
6. `cargo audit && cargo deny check` clean. The three new crates introduce no new external dep beyond what `[workspace.dependencies]` already pins.
7. `cargo machete` clean across the workspace, including the three new crates.
8. `kernex-runtime/Cargo.toml` reads `version = "0.6.0"`. `kernex-runtime/src/lib.rs` re-exports the five named symbols from `kernex_adapter_core`. A `cargo doc -p kernex-runtime --no-deps` build succeeds and the rendered docs show `kernex_runtime::Adapter`, `kernex_runtime::AdapterId`, `kernex_runtime::AdapterError`, `kernex_runtime::AdapterRegistry`, `kernex_runtime::Capability`.
9. `kernex-agent` build is unaffected by the runtime version bump in any way that would block a downstream `cargo update -p kernex-runtime`. Verified by a local check-out of `kernex-agent` `main` with the workspace's `kernex-runtime` patched to `path = "../kernex/crates/kernex-runtime"`: `cargo build` succeeds without source changes in `kernex-agent`.
10. No symbol collision. `kernex_runtime::Adapter` does not shadow any pre-existing public name in `kernex-runtime`.

## Risks

- **`BrainStore` trait surface churn when implementations land.** The forward-compatibility gamble is shipping stubs that turn out wrong: the first concrete `BrainStore` implementation discovers the trait method signatures need to differ, forcing a breaking change inside `kernex-brain` and a coordinated `kernex-runtime` bump downstream. Mitigation: the trait surface in this change is the absolute minimum (record and search method signatures only); the crate is `publish = false`, so any breaking change inside `kernex-brain` stays workspace-internal until it stabilizes. Reject any expansion of the surface mid-change. Expect the surface to change when the actual implementation lands; that change is owned by a follow-up.
- **Circular dep risk between `kernex-presets` and `kernex-adapter-core`.** The intended dep flow is presets to adapter-core, one-way. The risk is a maintainer adding `kernex-presets` to `kernex-adapter-core`'s `[dependencies]` for a "convenience" type, creating a cycle. Mitigation: a CI grep step in this change asserts that `crates/kernex-adapter-core/Cargo.toml` does not name `kernex-presets`. The grep lives in `Step 5: verification` and runs as part of pre-commit gate equivalents.
- **Workspace member count grows from 8 (7 crates plus `bench`) to 11 (10 crates plus `bench`).** Each new member adds compile time and CI matrix surface. Mitigation: the three new crates are tiny by design (one trait, one loader, one stub). Their per-crate compile time is negligible. The CI matrix already runs `cargo build --workspace`; no new matrix legs are added by this change.
- **Cold-start regression from new crate compilation.** `cargo build --workspace` cold builds three more crates than today. Mitigation: each new crate's source is small enough that the cold-start delta is dominated by `[workspace.dependencies]` resolution, which already accounts for `thiserror`, `async-trait`, `toml`, `serde`. Measure cold-start in `Step 5: verification` and surface any regression in the PR description.
- **`kernex-runtime` semver-minor bump might break downstream consumers.** The bump is `0.5.x` to `0.6.0`, which is a breaking change under Cargo's semver convention for pre-1.0 crates (any `0.x` to `0.y` increment is breaking). Mitigation: the only public-API delta is **additive** (five new re-exported symbols). No existing symbol is removed, renamed, or signature-changed. The bump is the cleanest way to surface "new public surface" through the version number; a downstream consumer that ignores the bump and stays on `0.5.x` continues to work without the new symbols. This is documented in the runtime crate's `CHANGELOG.md` as part of the change.
- **Re-export collision.** A consumer that already imports `kernex_runtime::*` could see a name collision against a hand-written `Adapter` or `AdapterId` symbol. Mitigation: grep `crates/kernex-runtime/src/` and the workspace for any pre-existing symbol with one of the five re-exported names. If found, the re-export uses an alias (`pub use kernex_adapter_core::Adapter as RuntimeAdapter;` style) and the alias choice is documented in the changelog. Verified at Step 3 in `tasks.md`.
- **`#[non_exhaustive]` on `AdapterError` and `BrainError` forces wildcard arms downstream.** Consumers `match`ing on either error type must include a wildcard arm or the compiler errors. Mitigation: this is the intended trade-off (variant additions stay non-breaking); document it in each crate's top-level rustdoc.

## Pre-implementation findings

Three findings recorded after the pre-change audit of the existing workspace shape, the existing `kernex-core` trait patterns, and the existing `kernex-providers` factory pattern. Each finding has a chosen mitigation for this change. The findings update posture but do not expand scope.

### Finding 1. Workspace member count breaches a soft 8-crate limit

This repo's prior change discipline holds the workspace to a soft ceiling of 8 member crates (the current 7 publishable crates plus `bench`). Going to 10 publishable members plus `bench` lifts the count above that ceiling.

The breach is allowed here because each of the three new crates has at least two consumers within the workspace once the follow-up adapter implementation lands:

- `kernex-adapter-core` is consumed by `kernex-runtime` (this change, via re-export) and by every future adapter implementation in the workspace (six reserved slots; first one lands in a follow-up change at the `kernex-agent` repo against this workspace's `kernex-adapter-core`).
- `kernex-presets` is consumed by `kernex-runtime` (transitively, once a follow-up wires preset loading into the runtime's startup path) and by `kernex-agent` (via crates.io once a path is established).
- `kernex-brain` is consumed by `kernex-runtime` (once the first `BrainStore` implementation lands in a follow-up change) and by `kernex-agent` via the runtime's re-export.

Mitigation for this change: document the breach explicitly in this proposal so reviewers see the count growing was a deliberate decision, not drift. Phrase the rule as "every new workspace member must have at least two in-workspace or sister-repo consumers"; record each new crate's two-consumer justification in `crates/<name>/README.md` (where present) or in the crate-level `lib.rs` doc string. No further breaches without a written justification.

### Finding 2. Adapter trait should match the existing `kernex-core` `#[async_trait]` pattern

The existing trait surface in `crates/kernex-core/src/traits.rs` (`Provider`, `StreamingProvider`, `Summarizer`, `Store`) uses `#[async_trait::async_trait]` universally on every method, including pure-sync accessors like `Provider::name(&self) -> &str` and `Provider::requires_api_key(&self) -> bool`. The new `Adapter` trait could deviate by using AFIT (async fn in trait, stable since Rust 1.75 and within this workspace's MSRV of 1.74 for non-trait async; trait-position async fn requires 1.75) but that complicates the `Send + Sync` bounds that any `dyn Adapter` consumer needs.

Decision: the new `Adapter` trait uses `#[async_trait]` only on the two methods that perform I/O (`detect`, `install_command`). The capability accessors (`fn supports_detect(&self) -> bool`, etc.) are sync, with default implementations returning `false`, so a no-op adapter compiles without overrides. This keeps `dyn Adapter` straightforwardly object-safe and matches the existing workspace pattern of "async-trait for I/O, sync for accessors", while sparing the workspace from having to bump its MSRV to 1.75 for AFIT trait-position support.

Mitigation for this change: a one-line comment in `crates/kernex-adapter-core/src/lib.rs` near the trait definition documents the choice and points at this finding. A follow-up change can move to AFIT once the workspace MSRV moves to 1.75 or later; the trait surface stays source-compatible because `#[async_trait]` desugars to the same `Pin<Box<dyn Future + Send>>` shape that AFIT compiles to under `Send` bounds.

### Finding 3. `BrainStore` trait surface is a forward-compatibility gamble

Shipping a trait that no implementation exercises is a known footgun: the first concrete implementation reveals which method signatures were wrong, and every consumer downstream of the trait pays the breaking-change tax. The risk is non-trivial because `BrainStore` is intentionally generic ("memory-domain operations"), and that generality maximizes the surface area where the first implementation can diverge from the stub.

Mitigation for this change: ship the absolute minimum. Two method signatures only, both shaped after the most concrete operations the runtime composition is known to need (a record-style insert returning a result, and a search-style query returning a result). No iterator types. No transaction handles. No batching primitives. No retention or eviction methods. The crate is `publish = false`, so any breaking change inside `kernex-brain` stays workspace-internal. Reviewers reject any expansion of the trait surface in this change; the rule is "if you cannot point at the line in the runtime composition that needs the method, do not add the method".

Reject any expansion of the surface mid-implementation. Expect the surface to change when the actual implementation lands; that change is owned by a follow-up change, not this one.
