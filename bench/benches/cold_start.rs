/// Cold-start benchmarks for the runtime and the kernex-memory store.
///
/// Two groups:
///
/// 1. `cold_start::RuntimeBuilder::build` measures the wall-clock time from
///    calling `RuntimeBuilder::new()` through the awaited `build()` completion,
///    which includes data-dir creation and SQLite store initialization.
///    The blog post cites 12ms on Apple M-series.
///
/// 2. `cold_start::memory_search_cold_start` measures the library-level
///    cold-start search path: opening a pre-populated SQLite store via
///    `Store::new` and issuing one FTS5 search query. Target: p95
///    <= 50 ms on macOS aarch64 release builds. Recorded as an
///    informational measurement, not a hard CI gate.
///
/// Run:
///   cargo bench --bench cold_start
///
/// Environment:
///   Measured on Apple M-series (arm64), NVMe SSD, macOS 14+.
///   Results will vary on spinning disks or network-mounted storage
///   because both paths perform I/O (mkdir, SQLite open).
use criterion::{criterion_group, criterion_main, Criterion};
use kernex_core::config::MemoryConfig;
use kernex_core::message::{CompletionMeta, Request, Response};
use kernex_memory::{MemoryStore, Store};
use kernex_runtime::RuntimeBuilder;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const SEED_MESSAGE_COUNT: usize = 200;
const SEARCH_QUERY: &str = "auth";
const SEARCH_LIMIT: i64 = 10;

fn bench_cold_start(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("cold_start");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    group.bench_function("RuntimeBuilder::build", |b| {
        b.iter(|| {
            let dir = TempDir::new().expect("tempdir");
            let path = dir.path().to_str().expect("utf8 path").to_string();
            rt.block_on(async move {
                RuntimeBuilder::new()
                    .data_dir(&path)
                    .build()
                    .await
                    .expect("build")
            });
            // TempDir drops here, cleaning up after each sample.
        });
    });

    group.finish();
}

/// Seed a Store at `db_path` with N synthetic exchanges; about a third mention
/// the `SEARCH_QUERY` term so FTS5 has hits to rank.
async fn seed_store(db_path: &str) {
    let config = MemoryConfig {
        backend: "sqlite".to_string(),
        db_path: db_path.to_string(),
        ..Default::default()
    };
    let store = Store::new(&config).await.expect("seed store new");

    for i in 0..SEED_MESSAGE_COUNT {
        let mentions_query = i % 3 == 0;
        let user_text = if mentions_query {
            format!("How do I implement {SEARCH_QUERY} for sample {i}?")
        } else {
            format!("Routine sample message number {i}")
        };
        let assistant_text = if mentions_query {
            format!("Implement {SEARCH_QUERY} via the standard provider flow.")
        } else {
            format!("Acknowledged sample {i}.")
        };
        let sender = format!("user_{}", i % 4);
        let request = Request::text(&sender, &user_text);
        let response = Response {
            text: assistant_text,
            metadata: CompletionMeta::default(),
        };
        store
            .store_exchange("api", &request, &response, "bench")
            .await
            .expect("seed store_exchange");
    }
}

fn bench_memory_search_cold_start(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Seed once into a persistent tempdir reused across iterations: cold-start
    // means opening a populated DB from disk, not building one from scratch.
    let seed_dir = TempDir::new().expect("seed tempdir");
    let seed_path = seed_dir
        .path()
        .join("bench_store.db")
        .to_str()
        .expect("utf8 seed path")
        .to_string();
    rt.block_on(async { seed_store(&seed_path).await });

    let mut group = c.benchmark_group("cold_start");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    group.bench_function("memory_search_cold_start", |b| {
        b.iter(|| {
            let path = seed_path.clone();
            rt.block_on(async move {
                // Cold-start path: open the pre-populated store from disk + run
                // one FTS5 search. Targets p95 <= 50 ms on macOS aarch64
                // release builds. The dispatch path goes through
                // `&dyn MemoryStore::search_messages` so the bench
                // continues to validate the trait surface that downstream
                // consumers call into, not a bypassed direct-struct path.
                let config = MemoryConfig {
                    backend: "sqlite".to_string(),
                    db_path: path,
                    ..Default::default()
                };
                let store = Store::new(&config).await.expect("open store");
                let store_handle: Arc<dyn MemoryStore> = Arc::new(store);
                let _hits = store_handle
                    .search_messages(SEARCH_QUERY, "no-conv", "user_0", SEARCH_LIMIT, None)
                    .await
                    .expect("search");
            });
        });
    });

    group.finish();
}

criterion_group!(benches, bench_cold_start, bench_memory_search_cold_start);
criterion_main!(benches);
