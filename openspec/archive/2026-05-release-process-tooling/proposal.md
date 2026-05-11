# Proposal: Release process tooling

- **Status:** Draft v0.1
- **Author:** Jose Hurtado
- **Repo:** `kernex-dev/kernex`
- **Change ID:** `release-process-tooling`

## Operator friction

Today the `kernex` workspace ships 11 crates at the workspace-pinned version `0.6.0`. The release flow is manual: a release manager moves the `[Unreleased]` block in `CHANGELOG.md` to a versioned block, bumps `[workspace.package].version`, and runs the seven-crate `cargo publish` chain by hand in dependency order per `RELEASE_CHECKLIST.md` Step 3. Two recurring costs:

1. **CHANGELOG drift.** Engineers writing PRs do not always remember to add bullets to the `[Unreleased]` block. The release manager hand-curates the CHANGELOG entry at release time by reading commits since the last tag and writing bullets — work that conventional-commit metadata already captures and could be derived automatically.
2. **Manual publish-chain operation.** The dependency-ordered `cargo publish` chain is a checklist run, not an automated workflow. A typo in the order or a forgotten crate is recoverable but not pleasant.

## Solution overview

Adopt [`release-plz`](https://release-plz.dev/) as the release-tooling for the kernex workspace.

- New `release-plz.toml` at the workspace root pins all 7 publishable crates plus the `kernex` umbrella to the same version via `version_group = "kernex"`. The 3 workspace-internal crates (`kernex-adapter-core`, `kernex-presets`, `kernex-brain`) and the `bench/` member carry `publish = false` in their Cargo.toml and are explicitly disabled in `release-plz.toml` for clarity.
- New `.github/workflows/release-plz.yml` triggers on push to `main`. release-plz reads conventional commits since the last `v*` tag and opens or updates a draft Release PR with the proposed version bump and a Keep-a-Changelog-formatted CHANGELOG diff. The Release PR is a draft for human review; merging it pushes the `v{version}` tag.
- New `.github/workflows/publish-crates.yml` triggers on `v*` tag push. Runs the pre-publish gate (build, clippy, test, fmt) then publishes each crate in dependency order with a 30s wait between steps for crates.io index propagation.
- `RELEASE_CHECKLIST.md` Step 2 collapses to "merge the open Release PR; review the CHANGELOG diff before merging". Step 3 collapses to "the publish-crates.yml workflow runs automatically on tag push; watch for failures and run the remaining publishes manually if it halts mid-chain". The publish-order list stays in the doc as the manual fallback. Steps 4-7 (kernex-agent dep bump, kernex-web translation sync, SEO consistency, stale-grep audit) are unchanged.

The change is pure-additive at the discipline level. No existing release ships differently. The current `[Unreleased]` block in CHANGELOG.md is migrated forward by release-plz on the first run; only the future-PR pattern changes.

## Why release-plz

Conventional commits are already the project convention. `release-plz` reads them via `git-cliff` and generates Keep-a-Changelog-formatted entries with no contributor-side change. It supports workspace-pinned versioning via `version_group`, opens a Release PR with version + CHANGELOG diff for human review, and tags the merge commit so downstream publishing is a separate, auditable trigger. Active maintenance, dual Apache-2.0/MIT license, ~1,400 GitHub stars, weekly releases.

Two alternatives were considered and rejected:

- **`knope` + `.changeset/*.md` per PR.** Adds one extra markdown file per PR. The CHANGELOG narrative-quality gain is real but does not justify a new contributor convention when commit-message body discipline already covers the same need.
- **Bespoke Rust binary in `xtask/` consuming the `changesets` crate directly.** Justified at very large workspace scales (50+ crates, multi-engineer release teams). Not justified here.

## Scope

### In scope

1. `release-plz.toml` at workspace root with `version_group = "kernex"` for the 7 publishable crates plus the `kernex` umbrella; `release = false` overrides for the 3 workspace-internal crates.
2. `.github/workflows/release-plz.yml` — Release PR workflow on push to `main`. Permissions: `contents: write`, `pull-requests: write`.
3. `.github/workflows/publish-crates.yml` — `cargo publish` chain on `v*` tag push. Pre-publish gate runs first; publish steps run with 30s waits for index propagation.
4. `RELEASE_CHECKLIST.md` refresh — Steps 2-3 collapsed; Steps 4-7 unchanged.
5. SHA-pinned third-party actions matching existing workspace discipline (`actions/checkout`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, `release-plz/release-plz-action`).
6. Pre-commit gate green: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo fmt --check`, `cargo audit`, `cargo deny check`, `cargo machete`, plus `release-plz update --dry-run`.

### Out of scope

- Per-crate independent versioning. The workspace stays workspace-pinned through the next major release.
- Cross-repo coordinated release across `kernex-dev` and `kernex-agent`. Each repo keeps its own release cadence.
- Conventional-commit linting in CI (commitlint or equivalent). Already enforced socially via PR review; CI bot deferred unless a non-conformant commit slips through after this change goes live.

## Success criteria

The change ships when:

1. `release-plz.toml` committed at workspace root.
2. `release-plz.yml` and `publish-crates.yml` committed under `.github/workflows/` with all third-party actions SHA-pinned.
3. `RELEASE_CHECKLIST.md` Steps 2-3 refreshed; Steps 4-7 unchanged.
4. `release-plz update --dry-run` runs cleanly against `main` and emits a candidate Release PR diff.
5. All gates green on the merge candidate: build, clippy, tests, fmt, audit, deny, machete.

## Risks

- **release-plz publish-order behavior on a 7-crate dependency chain.** The publish workflow is explicitly written with the dependency order documented in `RELEASE_CHECKLIST.md`, not derived from release-plz. The release manager retains the manual fallback if any step fails.
- **First Release PR will be large.** No `v0.6.0` tag exists yet; the first Release PR will derive bullets from every conventional commit since `v0.5.0`. The release manager prunes the CHANGELOG diff before merging.
- **GitHub Action SHA pin maintenance.** SHA pins lock the specific reviewed code path; major-version refs (`@v3`, `@v4`) are not used. Updating action versions requires a follow-up PR with the new SHA documented.
