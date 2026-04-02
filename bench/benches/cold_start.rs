/// Measures RuntimeBuilder::build() latency (cold start).
///
/// The blog post claims a 12ms cold start vs ~2200ms for Node/Python-based
/// runtimes. This benchmark measures the wall-clock time from calling
/// RuntimeBuilder::new() through the awaited build() completion, which
/// includes data-dir creation and SQLite store initialization.
///
/// Run:
///   cargo bench --bench cold_start
///
/// Environment:
///   Measured on Apple M-series (arm64), NVMe SSD, macOS 14+.
///   Results will vary on spinning disks or network-mounted storage
///   because build() performs I/O (mkdir, SQLite open).
use criterion::{criterion_group, criterion_main, Criterion};
use kernex_runtime::RuntimeBuilder;
use std::time::Duration;
use tempfile::TempDir;

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

criterion_group!(benches, bench_cold_start);
criterion_main!(benches);
