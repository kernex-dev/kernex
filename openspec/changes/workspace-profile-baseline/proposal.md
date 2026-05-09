# Proposal: workspace profile + dependency hygiene baseline

> **Change ID:** `workspace-profile-baseline`
> **Author:** Jose Hurtado
> **Status:** Draft v0.1
> **Sprint window:** Sprint 1 (~5 working days)
> **Repo:** `kernex-dev/kernex` (this repo)

## Operator friction

`kernex.dev` advertises **Single Binary · No runtime dependencies · Under 15 MB**. That is a load-bearing claim. Today the workspace ships the runtime crates that the `kx` binary depends on, but:

1. **No `[profile.release]` block** in workspace root `Cargo.toml`. Default cargo release profile leaves significant size on the table (no LTO, no strip, no panic=abort, no opt-level=z).
2. **`[workspace.dependencies]` is incomplete**. Most shared deps are pinned, but several (e.g. `clap`, `regex`, `tracing-subscriber`) are still picked per-crate, risking diamond duplication and inflated build times.
3. **No `cargo bloat` baseline.** Without a recorded baseline, regressions per crate are invisible.
4. **No `cargo machete` audit.** Unused declared deps may be inflating compile time and binary size silently.
5. **No CI size gates.** A PR that adds a 2 MB dep merges with no warning.
6. **No benchmark suite for `kernex-memory` cold-start search.** This proposal sets a 50 ms threshold for the library-level search call as a proxy for the eventual `kx mem search` CLI cold-start; the measurement infrastructure does not exist yet.

This is the foundation. Without it, every subsequent sprint operates blind to size and performance.

## Solution overview

Apply the following engineering disciplines as pure config and audit work:

- **Profile**: a `[profile.release]` block tuned for binary size on macOS aarch64.
- **Dependency hygiene**: shared `[workspace.dependencies]` pins, feature tightening, `cargo machete` cleanup, `cargo bloat` baseline.
- **Workspace deduplication**: member crates use `{ workspace = true }` for shared deps.
- **CI gates**: a `size-gate.yml` workflow that runs `cargo bloat` diffing and `cargo machete` on every PR. Binary-size and per-feature-matrix size jobs are scaffolded but disabled here; they activate in the sister repo (`kernex-agent`) where the `kx` binary lives.
- **Library cold-start benchmark**: a `criterion` bench measuring `kernex-memory`'s search call as a proxy for `kx mem search` cold-start. Threshold 50 ms on macOS aarch64 release builds; informational only this sprint, promoted to a hard gate after three stable runs.

This is **pure config and audit work.** No new code. No new crates. No API changes. No behavior change to consumers.

## Scope

### In scope (this sprint)

1. Add `[profile.release]` block to workspace root `Cargo.toml`:
   - `lto = "fat"`
   - `codegen-units = 1`
   - `strip = "symbols"`
   - `panic = "abort"` (after the catch_unwind audit in P0-1)
   - `opt-level = "z"`
2. Add `[profile.release-fast]` profile (inherits release, opt-level = "s") as an internal benchmark fallback.
3. Expand `[workspace.dependencies]` with these pins (audit consumers first; do not silently downgrade or break feature usage):
   - `tokio` with feature-restricted set (`["rt-multi-thread", "macros", "fs", "process", "sync", "time"]`)
   - `reqwest` with `["rustls-tls"]` plus any features still consumed by member crates (currently `json`, `stream` for `kernex-providers`)
   - `clap` only if a member crate or example actually depends on it (audit before adding)
   - `regex` with `["std"]`, no Unicode (audit Unicode usage in members first)
   - `tracing-subscriber` with `["fmt", "env-filter"]`
   - other deps audited and tightened only where bloat reports show wins and no consumer breaks
4. Convert member crates to `{ workspace = true }` for shared deps.
5. Capture `cargo bloat` baseline:
   - `cargo bloat --release --crates -n 30 > docs/bloat-baseline-YYYY-MM-DD-crates.txt`
   - `cargo bloat --release -n 30 > docs/bloat-baseline-YYYY-MM-DD-functions.txt`
   - Commit both.
6. Run `cargo machete` and remove unused declared deps.
7. Audit dep features per the inventory above; tighten where bloat reports show wins.
8. Add CI workflow `.github/workflows/size-gate.yml` with four jobs:
   - `binary-size` job (kx default ≤ 15 MB hard, ≤ 13 MB soft warn). **Gated `if: false` in this repo**; the `kx` binary lives in the sister repo. Activated when copied to that repo.
   - `feature-matrix` job (3 variants: minimal, default, full). **Gated `if: false` in this repo** for the same reason.
   - `bloat` job (per-crate diff against the committed baseline, soft warn at >10% growth). **Active in this repo.**
   - `unused-deps` job (`cargo machete`). **Active in this repo.**
9. Author benchmark suite in the existing `bench/` member crate, measuring `kernex-memory`'s library-level search call:
   - New `bench/benches/cold_start.rs` using `criterion`.
   - Seeds an in-memory store with a representative observation count (e.g. 1000 entries).
   - Measures cold-start latency of `kernex-memory`'s search API directly (no CLI wrapping).
   - Records p50, p95, p99.
   - Threshold: p95 ≤ 50 ms on macOS aarch64 release builds. **Informational only this sprint.** Promotion to hard gate after three stable runs.

### Out of scope (deferred)

- Any change to public API of any crate.
- Any new crate.
- Trait promotion in `kernex-memory`.
- Any work targeting the `kx` binary in the sister repo.
- Provider feature audits beyond removing unused features (full audit deferred).
- Per-variant binary-size gates (live in the sister repo's CI).

### Cross-repo coordination

This change is single-repo only. The sister repo (`kx`) will reuse the size-gate workflow pattern when its feature flags land; this SDD ships the YAML scaffold for that to copy.

## Why this sprint, why this scope

- **Foundation, not feature.** Every subsequent sprint trusts the size budget and measurement infrastructure. Skipping this means flying blind.
- **No new code = low risk.** Pure config changes are easy to review and easy to revert.
- **Compounded savings.** Profile changes alone are estimated to yield 35-55% binary size reduction. This sprint may land the default workspace builds well under the 15 MB ceiling immediately, before any feature work in the sister repo.
- **CI gates buy peace of mind.** Once CI fails on regressions, the team can iterate on features without manual size policing.

## Success criteria

The change ships when:

1. Workspace `Cargo.toml` has the new profile block, the expanded `[workspace.dependencies]`, and member crates use `{ workspace = true }` for shared deps.
2. `cargo build --workspace` succeeds.
3. `cargo clippy --workspace -- -D warnings` clean.
4. `cargo test --workspace` green.
5. `cargo fmt --check` clean.
6. `cargo audit && cargo deny check` clean.
7. `cargo bloat` baseline committed to `docs/bloat-baseline-<date>-{crates,functions}.txt`.
8. `cargo machete` reports no unused declared deps (or each false positive is annotated in `[package.metadata.cargo-machete]`).
9. CI workflow `.github/workflows/size-gate.yml` is in place and the active jobs (`bloat`, `unused-deps`) pass against `main`.
10. `bench/benches/cold_start.rs` produces a baseline; the cold-start time is recorded (whether or not it is under 50 ms; the threshold check is not enforced as a hard gate yet, only measured).

## Risks

- **`panic = "abort"` breaks tests using `#[should_panic]`.** Mitigation: P0-1 audits before the flip. Worst case: keep `panic = "unwind"`, lose 5-10% size win, ceiling still defendable.
- **`opt-level = "z"` hurts hot-path performance > 50 ms cold-start.** Mitigation: the `release-fast` profile is the already-declared fallback; pivot if benchmark exceeds threshold.
- **Tightening `[workspace.dependencies]` features breaks a consumer.** Mitigation: P2-1 audits consumers BEFORE tightening; deps with active feature dependents stay at their current feature set.
- **`cargo machete` flags a dep that is conditionally used.** Mitigation: add `[package.metadata.cargo-machete] ignored = [...]` for false positives with a comment.
- **CI size-gate too strict, false-positives PRs.** Mitigation: `bloat` job is soft-warn only (advisory); `binary-size` and `feature-matrix` jobs are gated off in this repo and only activate in the sister repo.
