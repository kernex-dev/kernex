/// Measures peak RSS when running N concurrent RuntimeBuilder instances.
///
/// The blog post claims 24MB for 10 concurrent agents vs ~310MB for
/// equivalent Node.js runtimes. This benchmark spawns N independent
/// Runtime instances in parallel and records peak process RSS before
/// dropping them, giving a per-agent memory baseline.
///
/// This is NOT a Criterion throughput benchmark — it's a point-in-time
/// RSS snapshot. Criterion is used only for structure and reporting.
///
/// Run:
///   cargo bench --bench memory
///
/// Note: RSS includes shared library pages, so absolute numbers vary
/// across OS and toolchain versions. Compare relative ratios, not
/// absolute values, across runtimes.
use criterion::{criterion_group, criterion_main, Criterion};
use kernex_runtime::RuntimeBuilder;
use tempfile::TempDir;
use tokio::runtime::Builder as TokioBuilder;

fn rss_bytes() -> u64 {
    // Read /proc/self/status on Linux; use sysctl on macOS.
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let kb: u64 = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                return kb * 1024;
            }
        }
        0
    }
    #[cfg(target_os = "macos")]
    {
        // task_info via rusage — uses getrusage(2) which reports maxrss in bytes on macOS.
        let usage = unsafe {
            let mut u: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_SELF, &mut u);
            u
        };
        // macOS: ru_maxrss is bytes
        usage.ru_maxrss as u64
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

#[cfg(target_os = "macos")]
extern crate libc;

fn bench_concurrent_memory(c: &mut Criterion) {
    let rt = TokioBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("memory");
    group.sample_size(10);

    for n in [1usize, 5, 10] {
        group.bench_function(format!("{n}_concurrent_agents"), |b| {
            b.iter(|| {
                let dirs: Vec<TempDir> = (0..n).map(|_| TempDir::new().expect("tempdir")).collect();
                let paths: Vec<String> = dirs
                    .iter()
                    .map(|d| d.path().to_str().expect("utf8").to_string())
                    .collect();

                let rss_before = rss_bytes();

                let _runtimes: Vec<_> = rt.block_on(async {
                    let futs = paths.iter().map(|p| {
                        let p = p.clone();
                        async move {
                            RuntimeBuilder::new()
                                .data_dir(&p)
                                .build()
                                .await
                                .expect("build")
                        }
                    });
                    futures::future::join_all(futs).await
                });

                let rss_after = rss_bytes();
                let delta_mb = (rss_after.saturating_sub(rss_before)) as f64 / (1024.0 * 1024.0);

                // Print for visibility in --nocapture mode.
                eprintln!(
                    "  {n} agents: +{:.1} MB RSS (per-agent: {:.1} MB)",
                    delta_mb,
                    delta_mb / n as f64
                );

                // Explicitly drop so measurement is clean.
                drop(_runtimes);
                drop(dirs);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_concurrent_memory);
criterion_main!(benches);
