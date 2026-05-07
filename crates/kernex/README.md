# kernex

Umbrella crate for the [Kernex](https://github.com/kernex-dev/kernex) AI agent
runtime in Rust. Re-exports the public API of
[`kernex-runtime`](https://crates.io/crates/kernex-runtime); pick whichever
feels more natural in your `Cargo.toml`.

```toml
[dependencies]
kernex = "0.4"
```

## Quick start

```rust,ignore
use kernex::RuntimeBuilder;
use kernex::core::message::Request;
use kernex::providers::ollama::OllamaProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime = RuntimeBuilder::new()
        .data_dir("~/.my-agent")
        .build()
        .await?;

    let provider = OllamaProvider::from_config(
        "http://localhost:11434".into(),
        "llama3.2".into(),
        None,
    )?;

    let request = Request::text("user-1", "Hello!");
    let response = runtime.complete(&provider, &request).await?;
    println!("{}", response.text);

    Ok(())
}
```

## Features

The umbrella ships with whatever `kernex-runtime` defaults to (currently
`sqlite-store`). Cargo's workspace dependency model does not let an
umbrella crate forward feature toggles per-call cleanly, so if you need
a custom feature combination — for example `opentelemetry` enabled or
`sqlite-store` disabled — depend on `kernex-runtime` directly:

```toml
[dependencies]
kernex-runtime = { version = "0.4", default-features = false, features = ["opentelemetry"] }
```

## Sub-crates

If you only need part of the runtime, depend on the underlying crate
directly:

- [`kernex-core`](https://crates.io/crates/kernex-core) — types, traits, errors
- [`kernex-providers`](https://crates.io/crates/kernex-providers) — 11 AI provider backends
- [`kernex-memory`](https://crates.io/crates/kernex-memory) — SQLite-backed store
- [`kernex-skills`](https://crates.io/crates/kernex-skills) — skill loader
- [`kernex-pipelines`](https://crates.io/crates/kernex-pipelines) — multi-agent topology
- [`kernex-sandbox`](https://crates.io/crates/kernex-sandbox) — Seatbelt / Landlock
- [`kernex-runtime`](https://crates.io/crates/kernex-runtime) — the facade

## License

Apache-2.0 OR MIT, at your option.
