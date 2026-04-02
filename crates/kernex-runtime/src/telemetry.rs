//! OpenTelemetry integration for Kernex.
//!
//! Enable with the `opentelemetry` Cargo feature:
//!
//! ```toml
//! kernex-runtime = { version = "*", features = ["opentelemetry"] }
//! ```
//!
//! Then install your exporter and wire the OTel layer into a
//! `tracing_subscriber` stack before building the runtime:
//!
//! ```rust,ignore
//! use opentelemetry_otlp::WithExportConfig;
//! use tracing_subscriber::prelude::*;
//!
//! let tracer = opentelemetry_otlp::new_pipeline()
//!     .tracing()
//!     .with_exporter(
//!         opentelemetry_otlp::new_exporter()
//!             .http()
//!             .with_endpoint("http://localhost:4318"),
//!     )
//!     .install_batch(opentelemetry_sdk::runtime::Tokio)?;
//!
//! tracing_subscriber::registry()
//!     .with(tracing_opentelemetry::layer().with_tracer(tracer))
//!     .with(tracing_subscriber::fmt::layer())
//!     .init();
//!
//! // ... use RuntimeBuilder as normal ...
//!
//! // Flush and shut down on process exit:
//! kernex_runtime::telemetry::shutdown();
//! ```
//!
//! Key spans emitted by the runtime:
//!
//! | Span name           | Source                           | Fields                         |
//! |---------------------|----------------------------------|--------------------------------|
//! | `kernex.complete`   | `Runtime::complete()`            | `provider`, `sender`           |
//! | `kernex.stream`     | `Runtime::complete_stream()`     | `provider`, `sender`           |
//! | `kernex.run`        | `Runtime::run()`                 | `provider`, `sender`, `turns`  |

/// Flush and shut down the global OpenTelemetry tracer provider.
///
/// Call this before your process exits to ensure all pending spans are
/// exported to your backend.
pub fn shutdown() {
    opentelemetry::global::shutdown_tracer_provider();
}
