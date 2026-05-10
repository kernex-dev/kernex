# Tasks: Workspace crate split for adapter, preset, and brain surfaces

> **Reference:** [proposal.md](proposal.md).
> Each task is sized at roughly two focused hours. Change tag: `[s2-c]`.
> **Status:** Archived. Landed at `kernex-dev/kernex@53b5537` on 2026-05-10. See `proposal.md` post-merge notes for drifts.

## Coordination

Single-repo `kernex-dev/kernex` only. No paired runtime PR. Depends on `cargo-feature-graph` having landed at `kernex-dev/kernex-agent` (which reserved the adapter and preset cfg surface this change defines the trait body for) and on `workspace-profile-baseline` having landed in this repo (which expanded `[workspace.dependencies]` and shipped the size-gate workflow templates).

Pre-commit gate (must pass on each commit, before any push):

```
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --check
```

For workspace-level verification at Step 5:

```
cargo audit
cargo deny check
cargo machete
```

No `Co-Authored-By` trailers. No `--no-verify`. No auto-commit. Each step's commit message follows conventional commits (`feat:`, `chore:`, `docs:`).

## Step 0: pre-execution audit (gates Step 1)

### P0-1. Confirm precedent SDDs landed `[s2-c]`

Verify both prerequisite changes are landed before scaffolding:

- `kernex-dev/kernex` `main` carries `openspec/archive/2026-MM-workspace-profile-baseline/` (or the still-active `openspec/changes/workspace-profile-baseline/` if the archive step is pending). If neither path exists, halt: this change builds on the workspace `[workspace.dependencies]` shape that `workspace-profile-baseline` introduced.
- `kernex-dev/kernex-agent` `main` carries `openspec/archive/2026-05-cargo-feature-graph/`. If it does not, halt: this change defines the trait surface that `cargo-feature-graph`'s adapter slot reservations refer to.

### P0-2. Audit existing public symbols for re-export collision `[s2-c]`

Grep `crates/kernex-runtime/src/` and the rest of the workspace for any pre-existing public symbol named `Adapter`, `AdapterId`, `AdapterError`, `AdapterRegistry`, or `Capability`:

```
grep -rn -E '\bpub (struct|enum|trait|fn|use) (Adapter|AdapterId|AdapterError|AdapterRegistry|Capability)\b' crates/ --include='*.rs'
```

Expected output: zero matches. If any match exists, escalate before P3-2 (the runtime re-export step) and pick aliases for the colliding names. Document the alias choice in the runtime crate's `CHANGELOG.md` entry for `0.6.0`.

### P0-3. Confirm `[workspace.dependencies]` carries every dep the new crates need `[s2-c]`

The new crates need `thiserror`, `async-trait`, `serde`, `toml`. Inspect `Cargo.toml` `[workspace.dependencies]`:

- `thiserror = "2"` (present per `workspace-profile-baseline`).
- `async-trait = "0.1"` (present).
- `serde = { version = "1", features = ["derive"] }` (present).
- `toml = "0.8"` (present).

If any pin is missing, file a follow-up against this repo. Do not silently add new pins from inside the new crates.

### P0-4. Confirm pre-change baseline build is clean `[s2-c]`

Run the pre-commit gate against current `main`:

```
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --check
```

All four green before scaffolding starts. If any red, fix on `main` first; do not start scaffolding on a red baseline.

**What to verify before Step 1:** baseline gate green; no symbol collisions; both precedent SDDs present.

## Step 1: scaffold three new crates

Each new crate ships in its own atomic commit. Commit message form: `feat(<crate>): scaffold <crate> with <one-liner>`.

### P1-1. Author `crates/kernex-adapter-core/Cargo.toml` and `src/lib.rs` `[s2-c]`

Manifest shape:

```toml
[package]
name = "kernex-adapter-core"
version = { workspace = true }
edition = { workspace = true }
rust-version = { workspace = true }
license = { workspace = true }
repository = { workspace = true }
description = "Adapter trait surface for the kernex workspace"
publish = false

[dependencies]
async-trait = { workspace = true }
serde       = { workspace = true }
thiserror   = { workspace = true }

[lints]
workspace = true
```

`src/lib.rs` defines, in this order:

1. `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` to match the workspace clippy posture.
2. Crate-level `//!` doc string naming the crate's responsibility (adapter trait surface, `AdapterId` enum, `AdapterRegistry`, `new_adapter` factory, `default_registry` constructor) and noting it is workspace-internal for now.
3. `pub enum AdapterId { Claude, Codex, OpenCode, Cursor, Cline, Windsurf }` with `#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]`. Manual `Display` impl returning the kebab-case name (`"claude"`, `"codex"`, `"opencode"`, `"cursor"`, `"cline"`, `"windsurf"`). Manual `FromStr` impl with the inverse map; unknown strings return `AdapterError::UnknownId`.
4. `pub struct Detection { pub installed: bool, pub version: Option<String>, pub config_path: Option<PathBuf> }` with `#[derive(Clone, Debug, Default, Serialize, Deserialize)]`.
5. `pub enum Capability { Detect, Install, Config, Invoke }` with `#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]`.
6. `pub enum AdapterError` via `#[derive(Debug, thiserror::Error)] #[non_exhaustive]` with at least these variants: `UnknownId(String)`, `NotImplemented(AdapterId)`, `Io(#[from] std::io::Error)`. Variants documented in `///` doc strings.
7. `pub trait Adapter: Send + Sync` with the shape:
   ```
   #[async_trait::async_trait]
   pub trait Adapter: Send + Sync {
       fn id(&self) -> AdapterId;
       fn supports_detect(&self) -> bool { false }
       fn supports_install(&self) -> bool { false }
       fn supports_config(&self) -> bool { false }
       fn supports_invoke(&self) -> bool { false }
       async fn detect(&self) -> Result<Detection, AdapterError>;
       async fn install_command(&self) -> Result<String, AdapterError>;
   }
   ```
   The trait is object-safe (no generics, no associated types, no `Self: Sized` methods). `Arc<dyn Adapter>` is the canonical handle. A one-line comment near the trait body explains the `#[async_trait]` choice per Finding 2 in `proposal.md`.
8. `pub struct AdapterRegistry { adapters: HashMap<AdapterId, Arc<dyn Adapter>> }` with three public methods: `pub fn lookup(&self, id: AdapterId) -> Option<Arc<dyn Adapter>>`, `pub fn register(&mut self, adapter: Arc<dyn Adapter>)`, `pub fn ids(&self) -> impl Iterator<Item = AdapterId> + '_`. `Default` impl returns an empty registry.
9. `const DEFAULT_ADAPTER_IDS: &[AdapterId] = &[AdapterId::Claude, AdapterId::Codex, AdapterId::OpenCode, AdapterId::Cursor, AdapterId::Cline, AdapterId::Windsurf];`.
10. `pub fn new_adapter(id: AdapterId) -> Result<Arc<dyn Adapter>, AdapterError>` switch-arm factory mirroring the shape of `crates/kernex-providers/src/factory.rs::ProviderFactory::create`. In this change every arm returns `Err(AdapterError::NotImplemented(id))`. Concrete implementations land in a follow-up change.
11. `pub fn default_registry() -> Result<AdapterRegistry, AdapterError>` iterates `DEFAULT_ADAPTER_IDS`, calls `new_adapter` for each, and inserts the resulting `Arc<dyn Adapter>` into a fresh `AdapterRegistry`. In this change the function returns `Err(AdapterError::NotImplemented(AdapterId::Claude))` on the first arm; tests assert this is the expected behaviour at the trait-surface stage.
12. `#[cfg(test)] mod tests { ... }` smoke-tests: `AdapterId::FromStr` round-trips against `Display`, `new_adapter` returns `NotImplemented` for every `AdapterId`, `AdapterRegistry::default()` is empty, `default_registry()` returns the documented `NotImplemented` error.

### P1-2. Author `crates/kernex-presets/Cargo.toml`, `src/lib.rs`, and the five TOML stubs `[s2-c]`

Manifest shape:

```toml
[package]
name = "kernex-presets"
version = { workspace = true }
edition = { workspace = true }
rust-version = { workspace = true }
license = { workspace = true }
repository = { workspace = true }
description = "Preset loader for the kernex workspace"
publish = false

[dependencies]
kernex-adapter-core = { workspace = true }
serde     = { workspace = true }
thiserror = { workspace = true }
toml      = { workspace = true }

[lints]
workspace = true
```

`src/lib.rs` defines:

1. Crate-level `//!` doc string naming the crate's responsibility (TOML preset loader and `Preset` value type) and noting it is workspace-internal for now.
2. `pub struct Preset { pub adapters: Vec<AdapterId>, pub components: Vec<String> }` with `#[derive(Clone, Debug, Default, Serialize, Deserialize)]`. The `adapters` field uses `kernex_adapter_core::AdapterId`.
3. `pub enum PresetError` via `#[derive(Debug, thiserror::Error)] #[non_exhaustive]` with at least: `UnknownPreset(String)`, `Io(#[from] std::io::Error)`, `Parse(#[from] toml::de::Error)`.
4. `pub fn load_preset(name: &str) -> Result<Preset, PresetError>`. Looks up the named preset in a constant table mapping each known name to its bundled stub path under `crates/kernex-presets/presets/`. For an empty stub the function returns `Preset { adapters: vec![], components: vec![] }`. For an unknown name returns `PresetError::UnknownPreset(name.to_string())`.
5. `const KNOWN_PRESETS: &[&str] = &["full-kernex", "security-hardened", "airgapped-defense", "solo-dev", "ci-only"];`.
6. `#[cfg(test)] mod tests { ... }` smoke-tests: `load_preset("full-kernex")` returns `Preset::default()` against an empty stub; `load_preset("nope")` returns `PresetError::UnknownPreset`.

Stub files at `crates/kernex-presets/presets/`:

- `full-kernex.toml`
- `security-hardened.toml`
- `airgapped-defense.toml`
- `solo-dev.toml`
- `ci-only.toml`

Each stub contains exactly one comment line naming the preset, e.g. `# full-kernex preset stub. Body is filled by a follow-up change.`. No table headers. No keys. The loader is built to handle the empty-stub case as a valid `Preset::default()`.

### P1-3. Author `crates/kernex-brain/Cargo.toml` and `src/lib.rs` `[s2-c]`

Manifest shape:

```toml
[package]
name = "kernex-brain"
version = { workspace = true }
edition = { workspace = true }
rust-version = { workspace = true }
license = { workspace = true }
repository = { workspace = true }
description = "Workspace-internal trait surface for memory-domain operations in the kernex workspace"
publish = false

[dependencies]
async-trait = { workspace = true }
thiserror   = { workspace = true }

[lints]
workspace = true
```

`src/lib.rs` defines:

1. Crate-level `#![doc = "..."]` describing the crate as "scaffold; implementations land in a follow-up change". The doc explicitly notes the trait surface is the absolute minimum (record and search method signatures only) and that the surface is expected to change when the actual implementation lands. No prior-art names. No domain claims beyond "memory-domain operations".
2. `pub trait BrainStore: Send + Sync` with `#[async_trait::async_trait]` on the two I/O methods. Method signatures sized for a record-style insert and a search-style query, both returning `Result<(), BrainError>` and `Result<Vec<String>, BrainError>` respectively. The exact parameter types are deliberately small: a `&str` key and a `&str` body for record; a `&str` query for search. No iterator types. No transaction handles. No batching primitives.
3. `pub enum BrainError` via `#[derive(Debug, thiserror::Error)] #[non_exhaustive]` with at least: `NotImplemented`, `Io(#[from] std::io::Error)`.
4. `#[cfg(test)] mod tests { ... }` one smoke test: assert that `BrainError::NotImplemented` formats to a non-empty string. No trait-object construction test; no implementation of `BrainStore` ships in this change.

### P1-4. Run the pre-commit gate after each P1-* commit `[s2-c]`

After P1-1: `cargo build -p kernex-adapter-core && cargo clippy -p kernex-adapter-core --all-targets -- -D warnings && cargo test -p kernex-adapter-core && cargo fmt --check`.

After P1-2: same, with `-p kernex-presets`.

After P1-3: same, with `-p kernex-brain`.

Each crate must build, lint, and test green before its commit lands. Do not stack commits on top of a red gate.

**What to verify before Step 2:** all three crates compile standalone; their per-crate test smoke green; `cargo fmt --check` clean.

## Step 2: workspace integration

### P2-1. Update workspace `members` and `[workspace.dependencies]` `[s2-c]`

The existing `members = ["crates/*", "bench"]` glob already picks up the three new directories under `crates/`. Verify this is the case:

```
cargo metadata --format-version=1 | jq '.workspace_members | length'
```

Expected count: 11 (10 publishable crates plus `bench`). If the count is wrong, escalate before continuing.

Add the three new internal-crate entries to the workspace `[workspace.dependencies]` table so members opt in via `{ workspace = true }`:

```
kernex-adapter-core = { version = "0.1.0", path = "crates/kernex-adapter-core" }
kernex-presets      = { version = "0.1.0", path = "crates/kernex-presets" }
kernex-brain        = { version = "0.1.0", path = "crates/kernex-brain" }
```

The version numbers start at `0.1.0` because the three crates are workspace-internal and do not inherit `version = { workspace = true }` from `[workspace.package]` (the workspace version stays `0.5.0` for the existing publishable crates aside from the runtime; `kernex-runtime` moves to `0.6.0` in P2-3).

### P2-2. Confirm one-way dep flow `[s2-c]`

Run:

```
cargo tree -p kernex-adapter-core | grep -E 'kernex-(presets|brain|runtime)'
cargo tree -p kernex-brain        | grep -E 'kernex-(adapter-core|presets|runtime)'
cargo tree -p kernex-presets      | grep -E 'kernex-(brain|runtime)'
```

Expected output: zero matches for each. `kernex-presets` may show `kernex-adapter-core` as a transitive dep, which is the documented one-way flow.

### P2-3. Bump `kernex-runtime` from 0.5.x to 0.6.0 `[s2-c]`

Edit `crates/kernex-runtime/Cargo.toml`:

```
version = "0.6.0"
```

Edit the workspace root `Cargo.toml` `[workspace.dependencies]` entry:

```
kernex-runtime = { version = "0.6.0", path = "crates/kernex-runtime" }
```

The other internal-crate version pins under `[workspace.dependencies]` (`kernex-core`, `kernex-providers`, `kernex-memory`, `kernex-skills`, `kernex-pipelines`, `kernex`, `kernex-sandbox`) stay on `0.5.0` for this change. The runtime moves to `0.6.0` alone because it is the only crate that gains a public surface delta in this change.

If any in-workspace consumer of `kernex-runtime` exists today (the workspace `members` list does not currently show one), update its dep entry to `0.6.0`. Run `grep -rn 'kernex-runtime' crates/*/Cargo.toml` to confirm.

Update `crates/kernex-runtime/CHANGELOG.md` (if present) or create it, with an entry under `## [0.6.0]` naming the additive re-export and the Cargo semver convention used for the bump.

### P2-4. Add `kernex-adapter-core` to the runtime's `[dependencies]` `[s2-c]`

Edit `crates/kernex-runtime/Cargo.toml` `[dependencies]`:

```
kernex-adapter-core = { workspace = true }
```

No other dep in the runtime changes. Confirm:

```
cargo build -p kernex-runtime
```

succeeds.

### P2-5. Add the re-export line in `crates/kernex-runtime/src/lib.rs` `[s2-c]`

Insert at the top of `crates/kernex-runtime/src/lib.rs`, after the existing crate-level doc string:

```
pub use kernex_adapter_core::{Adapter, AdapterId, AdapterError, AdapterRegistry, Capability};
```

If P0-2 surfaced a collision against any of the five names, replace the colliding name with an alias (`pub use kernex_adapter_core::Adapter as RuntimeAdapter;` style) and document the choice in the changelog entry from P2-3.

Confirm:

```
cargo doc -p kernex-runtime --no-deps
```

succeeds and the rendered docs show all five symbols (or their aliases) at the top level of `kernex_runtime`.

### P2-6. Run the workspace gate after Step 2 commits land `[s2-c]`

```
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --check
```

All four green. If any red, do not move to Step 3.

**What to verify before Step 3:** workspace member count is 11; one-way dep flow holds; runtime at `0.6.0`; runtime doc build shows the re-exports.

## Step 3: code surface verification

### P3-1. Grep new crates' source against the maintainer-side identifier deny-list `[s2-c]`

This SDD lives in a public repo. The three new crates' source must not name any prior-art memory-system identifiers. The deny-list itself is maintained out-of-band by the maintainer (so this public file does not enumerate the forbidden tokens) and is run against:

```
crates/kernex-adapter-core/  crates/kernex-presets/  crates/kernex-brain/
  --include='*.rs' --include='*.toml' --include='*.md'
```

Expected output: zero matches. If any match exists, halt and rewrite the offending source.

### P3-2. Confirm runtime re-export round-trips `[s2-c]`

Add a smoke test inside `crates/kernex-runtime/tests/adapter_reexport.rs` that uses `kernex_runtime::AdapterId::Claude` and `kernex_runtime::AdapterError`. The test asserts the re-export path compiles and that the symbol identities match `kernex_adapter_core`'s. Example shape:

```
#[test]
fn reexport_paths_compile() {
    let id: kernex_runtime::AdapterId = kernex_runtime::AdapterId::Claude;
    let _err: kernex_runtime::AdapterError = kernex_runtime::AdapterError::NotImplemented(id);
}
```

`cargo test -p kernex-runtime --test adapter_reexport` green.

### P3-3. Grep runtime source for accidental wildcard re-exports `[s2-c]`

Confirm the re-export added in P2-5 is explicit (named symbols only) and that no `pub use kernex_adapter_core::*;` exists. Run:

```
grep -rn 'pub use kernex_adapter_core' crates/kernex-runtime/src/
```

Expected output: exactly one match, the line added in P2-5, and that line names the five symbols explicitly. No wildcard.

### P3-4. Confirm `cargo machete` clean across the workspace `[s2-c]`

```
cargo machete
```

Expected output: zero unused declared deps. If `cargo machete` flags any dep in the three new crates (because the smoke tests do not exercise every path in this change), add to that crate's `[package.metadata.cargo-machete] ignored = [...]` with a one-line comment naming the symbol that consumes it. Do not silently remove a dep the crate's public surface needs.

**What to verify before Step 4:** prior-art grep clean; runtime re-export round-trips in tests; no wildcard re-exports; machete clean.

## Step 4: CI matrix

This change does not introduce a new workflow file. The existing `.github/workflows/ci.yml` (workspace build, clippy, test, fmt) and `.github/workflows/size-gate.yml` (`bloat`, `unused-deps` jobs active in this repo) cover the three new crates because they run `--workspace`.

### P4-1. Confirm CI picks up the three new crates `[s2-c]`

Open a draft PR with the Step 1 and Step 2 commits. Confirm:

- `cargo build --workspace` job is green and shows the three new crates' compile output in its log.
- `clippy --workspace` job is green.
- `test --workspace` job is green and lists the new crates' test counts.
- `fmt --check` job is green.
- `cargo audit` and `cargo deny check` jobs are green.
- `cargo machete` job is green.
- `bloat` job runs against the workspace and emits no per-crate growth warning above 10 percent (the three new crates add a small amount of compile-time overhead but no measurable binary-size delta because none of them is reachable from any binary in this change).

If any job is red, halt and fix before requesting review.

### P4-2. Confirm size-gate workflow templates remain inert in this repo `[s2-c]`

The `binary-size` and `feature-matrix` jobs in `.github/workflows/size-gate.yml` are guarded with `if: contains(github.repository, 'kernex-agent')`. They stay inert in this repo. This change does not flip those guards. Confirm by inspecting the workflow run summary on the draft PR: the two guarded jobs report as skipped, not failed.

**What to verify before Step 5:** all active CI jobs green on the draft PR; guarded jobs skipped, not failed.

## Step 5: verification

### P5-1. Pre-commit gate against the merged Step 1 to Step 4 commits `[s2-c]`

```
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --check
```

All four green.

### P5-2. Workspace audit gate `[s2-c]`

```
cargo audit
cargo deny check
cargo machete
```

All three green. The three new crates introduce no new external dep beyond what `[workspace.dependencies]` already pins, so the audit gate is expected to be unchanged from the pre-change baseline.

### P5-3. Size-gate impact on `kx` via `kernex-runtime` transitive `[s2-c]`

The new crates are reachable from `kernex-runtime` via the re-export, but no symbol from any new crate is invoked at runtime (every `Adapter` factory arm returns `NotImplemented`). Verify the runtime's reachable-code footprint:

```
cargo tree -p kernex-runtime --depth 1
```

Expected delta versus pre-change: exactly one new line, `kernex-adapter-core`. No other dep change.

Build the headline `kx` binary against this workspace via a local check-out of `kernex-agent` patched to `path = "../kernex/crates/kernex-runtime"`:

```
# inside a local kernex-agent check-out, with kernex-runtime patched to the path
cargo build --release
ls -lh target/release/kx
```

Record the size in the PR description and compare against the post-`workspace-profile-baseline` baseline at `docs/bloat-baseline-*.txt`. Net-new binary-size delta should be in the noise (under 100 KB) because the new crates contribute only the trait-surface symbols and no reachable code.

### P5-4. Cross-repo smoke check against `kernex-agent` `[s2-c]`

Inside the local `kernex-agent` check-out from P5-3, run:

```
cargo build
cargo build --no-default-features --features memory-cli
cargo test
```

All three green without source changes inside `kernex-agent`. The runtime version bump is mechanically additive; no `kernex-agent` code change is required.

### P5-5. Confirm `cargo doc` builds the runtime re-exports cleanly `[s2-c]`

```
cargo doc -p kernex-runtime --no-deps
```

Inspect the generated rustdoc for `kernex_runtime`. The crate-level page lists `Adapter`, `AdapterId`, `AdapterError`, `AdapterRegistry`, `Capability` (or their aliases per P0-2) under "Re-exports". No broken intra-doc links (the workspace lints already deny those at the workspace level).

### P5-6. Confirm the workspace builds at the new member count `[s2-c]`

```
cargo metadata --format-version=1 | jq '.packages | map(select(.source == null)) | length'
```

Expected output: 11. Workspace members: the seven existing publishable crates plus `bench`, plus `kernex-adapter-core`, `kernex-presets`, `kernex-brain`. If the count differs, escalate.

**What to verify before Step 6:** every gate above green; size delta in the noise; cross-repo smoke clean; rustdoc clean; member count is 11.

## Step 6: archive

### P6-1. Archive the change inside this repo `[s2-c]`

After this change merges, move:

```
openspec/changes/workspace-crate-split/
  to
openspec/archive/2026-MM-workspace-crate-split/
```

Replace `MM` with the merge month. Add a one-line header to each archived file noting the merge date and commit SHA.

### P6-2. Note any deferred decisions `[s2-c]`

If the trait surface in `kernex-adapter-core` or `kernex-brain` shipped with any deviation from the proposal (e.g. an extra capability flag added during implementation, an alias used per P0-2), document the deviation in the archived `proposal.md` "Risks" section. The archive is the source of truth for what landed.

## What is intentionally absent

- Concrete adapter implementations for any of the six `AdapterId` variants. The trait is defined; bodies are not.
- Preset TOML bodies. The five stub files exist as empty headers; their content is filled in a follow-up change.
- `BrainStore` implementations. The trait surface is shipped as a forward-compatibility scaffold.
- Per-provider feature flags inside `kernex-providers`. The native provider matrix stays unconditional.
- Any change to `kernex-agent` source. The runtime version bump is mechanically additive and `kernex-agent` is not modified by this change.
- Crates.io publishing for any of the three new crates. All three start as `publish = false`. They are workspace-internal until an external consumer emerges.
- Trait promotion or signature change for `Provider`, `StreamingProvider`, `Summarizer`, `Store` in `kernex-core`.
- Removal or rename of any existing module or symbol.
- Any size-gate threshold change. The 15 MB hard gate against `kx` (in `kernex-agent`) and the workspace-internal `bloat` soft warn (in this repo) stay at their current thresholds.
