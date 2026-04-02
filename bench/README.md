# kernex-bench

Criterion benchmarks for the Kernex runtime.

These benchmarks back the numbers cited in
[Why We Built Kernex in Rust](https://kernex.dev/blog/why-rust-for-ai-agents/).

## Benchmarks

### `cold_start`

Measures `RuntimeBuilder::build()` wall-clock time: from calling
`RuntimeBuilder::new()` through the awaited `build()` completion.

This includes:

- Data directory creation (`tokio::fs::create_dir_all`)
- SQLite store initialization (`Store::new`, schema migration)

The blog post cites **12ms** on Apple M-series hardware (NVMe, macOS 14).
The comparison baseline (~2200ms) comes from running equivalent Node.js and
Python agent frameworks through the same initialization path.

### `memory`

Measures peak RSS delta when 1, 5, and 10 `Runtime` instances are alive
concurrently. Instances are built in parallel using `futures::future::join_all`.

The blog post cites **24MB total** for 10 concurrent agents. RSS is read from
`/proc/self/status` (Linux) or `getrusage(RUSAGE_SELF)` (macOS).

**Note:** RSS includes shared library pages mapped once per process. The
delta-from-baseline approach accounts for this, but absolute numbers vary
across OS and toolchain versions. Use relative ratios for cross-runtime
comparisons.

## Running

```bash
# All benchmarks
cargo bench

# Single benchmark
cargo bench --bench cold_start
cargo bench --bench memory

# Print RSS deltas to stderr
cargo bench --bench memory -- --nocapture
```

HTML reports are written to `../target/criterion/`.

## Environment

Reference results were collected on:

- Hardware: Apple M-series (arm64), NVMe SSD
- OS: macOS 14 (Sonoma)
- Rust: stable, release profile
- SQLite: bundled (sqlx default)

Results will be higher on spinning disks or network-mounted storage because
`build()` performs file I/O.
