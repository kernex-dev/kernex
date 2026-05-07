# kernex-runtime

Top-level runtime that ties the [Kernex](https://github.com/kernex-dev/kernex) workspace together.

Most consumers should start here. `RuntimeBuilder` composes:

- A `Provider` from [`kernex-providers`](https://crates.io/crates/kernex-providers) (or your own)
- The `Store` from [`kernex-memory`](https://crates.io/crates/kernex-memory) (SQLite-backed history, facts, recall, outcomes, token usage)
- Skill loading and trigger matching from [`kernex-skills`](https://crates.io/crates/kernex-skills)
- Tool execution with sandbox enforcement from [`kernex-sandbox`](https://crates.io/crates/kernex-sandbox)
- Multi-agent pipelines from [`kernex-pipelines`](https://crates.io/crates/kernex-pipelines)

A typical request flows: build context from memory → enrich with matched skills → call provider → persist exchange → record token usage. `Runtime::complete` and `Runtime::complete_stream` cover the standard and streaming paths; `Runtime::run` chains a `RunConfig` of multiple steps.

## Quick start

```rust,ignore
use kernex_runtime::RuntimeBuilder;
use kernex_core::message::Request;

let runtime = RuntimeBuilder::new()
    .data_dir("~/.kernex")
    .system_prompt("You are a helpful assistant.")
    .build()
    .await?;

let request = Request::text("user-1", "Hello!");
let response = runtime.complete(&provider, &request).await?;
println!("{}", response.text);
```

## Documentation

- API reference: <https://docs.rs/kernex-runtime>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
