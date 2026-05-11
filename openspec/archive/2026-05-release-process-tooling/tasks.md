# Tasks: Release process tooling

> **Reference:** [proposal.md](proposal.md). Each task is sized at roughly two focused hours.

---

## Step 0 — Pre-execution audit

### P0-1. Confirm baseline build is clean

- `cargo build --workspace --all-targets` succeeds.
- `cargo test --workspace` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.

**Verification:** all four commands exit 0 on a freshly cloned `main`.

### P0-2. Confirm no live release-plz config exists

- `find . -name 'release-plz.toml'` returns no matches at workspace root.
- `grep -lE 'release-plz' .github/workflows/` returns no match.

**Verification:** the workspace has no pre-existing release-tool config that would conflict with new files added in Step 1.

### P0-3. Confirm workspace versioning invariant

- `grep '^version' Cargo.toml` shows `version = "0.6.0"` (or current) under `[workspace.package]`.
- `grep -E 'version\.workspace = true' crates/*/Cargo.toml` returns one match per publishable crate.

**Verification:** every member uses `version.workspace = true`. release-plz can rely on a single source of truth.

---

## Step 1 — Author the configuration

### P1-1. Create `release-plz.toml`

- New file `release-plz.toml` at workspace root.
- `[workspace] release = false` (Release-PR mode only; no auto-publish).
- `[workspace] changelog_update = true` (maintain CHANGELOG.md per Keep-a-Changelog).
- `[workspace] git_tag_enable = true` with `git_tag_name = "v{{ version }}"`.
- `[workspace] git_release_enable = false` (release manager creates GitHub releases manually if desired).
- One `[[package]]` block per publishable crate plus the `kernex` umbrella with `version_group = "kernex"`.
- One `[[package]] release = false` override per workspace-internal crate (`kernex-adapter-core`, `kernex-presets`, `kernex-brain`).

**Verification:** `release-plz update --dry-run` runs from workspace root without errors. Output mentions the 8 publishable crates and the workspace-pinned bump candidate.

### P1-2. Create `.github/workflows/release-plz.yml`

- New file `.github/workflows/release-plz.yml`.
- Triggers on push to `main`.
- Permissions: `contents: write`, `pull-requests: write`.
- `concurrency: release-plz-${{ github.ref }}` with `cancel-in-progress: false` (do not cancel an in-flight Release PR update).
- Steps: checkout (full history), Rust toolchain, rust-cache, release-plz-action with `command: release-pr`.
- All third-party actions SHA-pinned with version comment matching existing workflow discipline.

**Verification:** `actionlint` clean. The workflow's permission block is the minimum required.

### P1-3. Create `.github/workflows/publish-crates.yml`

- New file `.github/workflows/publish-crates.yml`.
- Triggers on push of tags matching `v[0-9]+.[0-9]+.[0-9]+`.
- Permissions: `contents: read`.
- `gate` job: pre-publish checks (build, clippy, test, fmt).
- `publish` job: depends on `gate`. Runs `cargo publish` for each crate in this order with `sleep 30` between steps:
  1. `kernex-core`
  2. `kernex-sandbox`
  3. `kernex-memory`
  4. `kernex-pipelines`
  5. `kernex-skills`
  6. `kernex-providers`
  7. `kernex-runtime`
  8. `kernex` (umbrella)
- `CARGO_REGISTRY_TOKEN` secret referenced via `secrets.CARGO_REGISTRY_TOKEN`.

**Verification:** `actionlint` clean. The publish-order list matches `RELEASE_CHECKLIST.md` Step 3 verbatim.

### P1-4. Refresh `RELEASE_CHECKLIST.md`

- Edit `RELEASE_CHECKLIST.md`.
- Step 2: collapse to "open the release-plz Release PR; review the CHANGELOG diff; merge to push the tag". Manual sub-tasks for files release-plz does not touch (`SECURITY.md`, `README.md`, `MANIFESTO.md` updates, stale-count grep) stay verbatim.
- Step 3: collapse to "the publish-crates.yml workflow runs automatically on tag push; watch for failures and run the remaining publishes manually if it halts mid-chain". Publish-order list stays as the manual fallback.
- Steps 4, 5, 6, 7: unchanged.

**Verification:** `diff` against `main`'s `RELEASE_CHECKLIST.md` shows ONLY changes to Step 2 and Step 3 plus the new pointer to release-plz. No edits in Steps 4-7.

---

## Step 2 — Verification gate

### P2-1. Workspace pre-commit gate

- `cargo build --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo test --workspace` passes.
- `cargo fmt --all --check` clean.
- `cargo audit` clean (or only pre-existing accepted entries).
- `cargo deny check` clean.
- `cargo machete` clean.

**Verification:** all seven exit 0 on the merge candidate.

### P2-2. release-plz dry-run

- `release-plz update --dry-run` runs from workspace root. Captures the candidate Release PR diff.

**Verification:** dry-run output captured into the PR description; first Release PR after merge will be reviewable.

### P2-3. Workflow lint

- `actionlint` (or equivalent) over `.github/workflows/release-plz.yml` and `.github/workflows/publish-crates.yml`.

**Verification:** clean. Both workflows parse and reference valid action SHAs.

---

## Step 3 — Archive

### P3-1. Move the change directory to archive

- After merge, move `openspec/changes/release-process-tooling/` to `openspec/archive/2026-MM-release-process-tooling/`.
- Confirm `proposal.md` and `tasks.md` carry over.
- Add a "Post-merge notes" section to the archived `proposal.md` recording the merge SHA and any drifts vs the spec.

**Verification:** `ls openspec/archive/ | grep release-process-tooling` returns the archived directory. `ls openspec/changes/release-process-tooling/` no longer exists.

---

## Done criteria

- `release-plz.toml` committed and validated via `release-plz update --dry-run`.
- `release-plz.yml` and `publish-crates.yml` committed and `actionlint`-clean.
- `RELEASE_CHECKLIST.md` Steps 2-3 collapsed; Steps 4-7 unchanged.
- All gates green on merge candidate.
- Change directory archived under `openspec/archive/2026-MM-release-process-tooling/`.
