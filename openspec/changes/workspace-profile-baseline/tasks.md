# Tasks: workspace profile + dependency hygiene baseline

> **Reference:** [proposal.md](proposal.md).
> Each task ≤ 2 focused hours. Sprint tag: `[s1]`.

## Coordination

This change is single-repo only. The sister repo (`kx`) will follow with its own feature-flag SDD that depends on `[workspace.dependencies]` being expanded here.

## Phase 0: pre-execution audit (gates Phase 1)

### P0-1. Audit `catch_unwind` and `#[should_panic]` `[s1]`

```bash
grep -rn "catch_unwind\|should_panic\|panic::set_hook\|panic::take_hook\|resume_unwind\|UnwindSafe\|AssertUnwindSafe" \
  crates/ examples/ bench/ --include="*.rs"
```

- If there are NO production callers of `catch_unwind` and `#[should_panic]` is only in test code, proceed with `panic = "abort"` in P1-1.
- If production callers exist, document them in [proposal.md](proposal.md) "Risks", keep `panic = "unwind"`, and proceed.

## Phase 1: profile config

### P1-1. Add `[profile.release]` block to workspace `Cargo.toml` `[s1]`

```toml
[profile.release]
lto           = "fat"
codegen-units = 1
strip         = "symbols"
panic         = "abort"  # only if P0-1 cleared
opt-level     = "z"
```

If P0-1 did not clear `panic = "abort"`, omit that line and document.

### P1-2. Add `[profile.release-fast]` fallback profile `[s1]`

```toml
[profile.release-fast]
inherits      = "release"
opt-level     = "s"
codegen-units = 1
lto           = "fat"
strip         = "symbols"
panic         = "abort"  # match P1-1
```

### P1-3. Verify build clean `[s1]`

```bash
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo fmt --check
```

All four must pass before proceeding.

## Phase 2: workspace dependencies

### P2-1. Audit consumers, then expand `[workspace.dependencies]` `[s1]`

**Audit first.** For each candidate tightening, grep the existing consumers:

```bash
# For each member crate Cargo.toml, list the features it pulls on each shared dep.
grep -E '^[a-zA-Z0-9_-]+ ?=' crates/*/Cargo.toml | grep -E '(tokio|reqwest|tracing-subscriber|chrono|regex|clap)'
```

Tighten only what no consumer actively depends on. Keep additive feature sets where any consumer needs the feature. Suggested target shape (adjust based on audit):

```toml
[workspace.dependencies]
tokio = { version = "1", default-features = false, features = ["rt-multi-thread", "macros", "fs", "process", "sync", "time"] }
tokio-util = { version = "0.7", features = ["codec"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "env-filter"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
# Add "json" / "stream" back if kernex-providers (or any other crate) consumes them.
regex = { version = "1", default-features = false, features = ["std"] }
# Add "unicode-perl" / etc. back if any crate uses Unicode-aware regex.
chrono = { version = "0.4", default-features = false, features = ["serde"] }
async-trait = "0.1"
```

**Do not** add `clap` to `[workspace.dependencies]` unless an `examples/` crate or test fixture in this workspace consumes it (none do at time of writing; the binary lives in the sister repo).

Adjust versions to match what member crates currently pin. **Never silently downgrade.**

### P2-2. Convert member crates to `{ workspace = true }` `[s1]`

For each crate in `crates/*`:

- Find direct deps in their `Cargo.toml` that overlap with `[workspace.dependencies]`.
- Replace with `dep = { workspace = true }`.
- Preserve any local feature additions via `features = ["x"]`.

Verify each crate still builds standalone:

```bash
cargo build -p kernex-core
cargo build -p kernex-runtime
# ...etc.
```

### P2-3. `cargo machete` audit `[s1]`

```bash
cargo install cargo-machete  # if not already installed
cargo machete
```

For each unused dep flagged: remove from the crate's `Cargo.toml`, OR add to `[package.metadata.cargo-machete] ignored = [...]` with a comment if conditionally used.

### P2-4. Verify build clean again `[s1]`

Same gate as P1-3. Expected outcome: same green status, smaller compile times due to dedup.

## Phase 3: bloat baseline

### P3-1. Capture `cargo bloat --crates` baseline `[s1]`

```bash
mkdir -p docs
cargo install cargo-bloat  # if not already installed
cargo bloat --release --crates -n 30 > docs/bloat-baseline-$(date +%Y-%m-%d)-crates.txt
cargo bloat --release -n 30 > docs/bloat-baseline-$(date +%Y-%m-%d)-functions.txt
```

Commit both files. They are the reference for future bloat-diff CI gates.

### P3-2. Compare against pre-profile baseline (optional, info only) `[s1]`

If a snapshot of the pre-profile binary exists, diff the bloat outputs to confirm the expected 35-55% reduction. If no pre-snapshot exists, skip.

## Phase 4: CI gates

### P4-1. Author `.github/workflows/size-gate.yml` `[s1]`

Four jobs:

1. `binary-size` — runs on `macos-latest`, builds the default release binary, fails > 15 MB. **Gated `if: false` in this repo** (no binary here). Activates when copied to the sister repo.
2. `feature-matrix` — runs on `ubuntu-latest`, builds 3 variants (minimal / default / full). **Gated `if: false` in this repo** for the same reason.
3. `bloat` — runs `cargo bloat --release --crates`, diffs against `docs/bloat-baseline-*.txt`, soft-warns on > 10% per-crate growth. **Active.**
4. `unused-deps` — runs `cargo machete`. **Active.**

The two gated-off jobs ship as scaffolding so the sister repo can copy the workflow and flip them on by removing the `if: false` line.

### P4-2. Author `scripts/check-size.sh` helper `[s1]`

```bash
#!/usr/bin/env bash
# scripts/check-size.sh <variant-name> <max-bytes>
# Used by feature-matrix CI job to enforce per-variant ceilings.
```

Make it cross-platform (`stat -c%s` Linux, `stat -f%z` macOS).

### P4-3. Author `scripts/diff-bloat.py` helper `[s1]`

```bash
# scripts/diff-bloat.py <baseline.txt> <current.txt>
# Soft-warns if any crate's contribution grew > 10%.
# Exit 0 in all cases (advisory only).
```

Python 3, no external deps.

## Phase 5: library cold-start benchmark

### P5-1. Add `criterion` to `bench/Cargo.toml` (if not already present) `[s1]`

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
```

Reuse the existing `bench` member crate.

### P5-2. Author `bench/benches/cold_start.rs` `[s1]`

This bench measures `kernex-memory`'s library-level search call as a proxy for the eventual `kx mem search` CLI cold-start (which lives in the sister repo).

- Sets up a `tempfile::TempDir` for the store path; each benchmark iteration starts with a fresh process state where possible.
- Seeds the store with a representative observation count (e.g. 1000 observations).
- Benchmarks the cold-start path: opening the store + issuing a search query (e.g. for "auth").
- Records p50, p95, p99 latency.
- Asserts on p95 informationally: target ≤ 50 ms on macOS aarch64 release builds.

For this sprint, the benchmark MEASURES; the CI assertion is informational only (does not block merges) until promoted to a hard gate after three stable runs.

### P5-3. Capture benchmark baseline `[s1]`

```bash
cargo bench --bench cold_start > docs/bench-baseline-$(date +%Y-%m-%d)-cold-start.txt
```

Commit.

## Phase 6: verify

### V-1. Full pre-commit gate `[s1]`

```bash
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo fmt --check
cargo audit && cargo deny check
```

All six green.

### V-2. CI workflow runs green on a noop PR `[s1]`

Open a noop PR (e.g. README typo fix). Confirm the new size-gate.yml workflow runs and the active jobs (`bloat`, `unused-deps`) pass.

### V-3. Archive `[s1]`

```bash
mv openspec/changes/workspace-profile-baseline/ \
   openspec/archive/2026-MM-workspace-profile-baseline/
```

Add merge date and commit SHA to each file's header.

## What is intentionally absent

- Any new crate.
- Any trait change in `kernex-memory`.
- Any work in the sister repo (the `kx` binary, feature flags, adapter implementations).
- Per-variant binary-size enforcement (deferred to the sister repo's CI when its feature flags land; this sprint ships the YAML scaffold but the binary-size and feature-matrix jobs are gated off here).
