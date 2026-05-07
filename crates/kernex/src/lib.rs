//! # kernex
//!
//! Umbrella crate for the [Kernex](https://github.com/kernex-dev/kernex) AI
//! agent runtime. Provides a single dependency for users who prefer
//! `cargo add kernex` over picking individual sub-crates by hand.
//!
//! Every public item is re-exported from
//! [`kernex_runtime`](https://docs.rs/kernex-runtime); see that crate's
//! documentation for the full API. Examples in `kernex-runtime` work
//! verbatim here by replacing the import path:
//!
//! ```rust,ignore
//! // Either of these works:
//! use kernex::RuntimeBuilder;
//! use kernex_runtime::RuntimeBuilder;
//! ```
//!
//! ## When to use this crate vs `kernex-runtime`
//!
//! - **`kernex`** — discoverability. `cargo add kernex` works without
//!   knowing the workspace layout. Crate-name-as-product.
//! - **`kernex-runtime`** — direct dependency, slightly shorter compile
//!   times in workspaces that already depend on it transitively.
//!
//! Either is fine; they expose the same API.
//!
//! ## Features
//!
//! Forwarded unchanged to `kernex-runtime`:
//!
//! - `sqlite-store` (default) — pulls in `kernex-memory` for persistent
//!   conversation, fact, and outcome storage.
//! - `opentelemetry` — enables OTLP export of runtime spans via
//!   `tracing-opentelemetry`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub use kernex_runtime::*;
