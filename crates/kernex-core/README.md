# kernex-core

Foundation types, traits, and error handling for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

This crate is the dependency-free root of the workspace. It defines:

- `Provider` and `HookRunner` traits — the integration points every other crate builds on
- `Context`, `Request`, `Response`, `CompletionMeta` — the shared message types
- `KernexError` — the unified error enum
- `PermissionRules` — declarative tool allow/deny patterns
- `GuardrailRunner` — pre/post-message safety hooks
- Built-in helpers for sanitization, pricing tables, and UTF-8 boundary handling

You usually do not depend on `kernex-core` directly. Pull in [`kernex-runtime`](https://crates.io/crates/kernex-runtime) (or the [`kernex`](https://crates.io/crates/kernex) umbrella) and its public surface re-exports what you need.

## Documentation

- API reference: <https://docs.rs/kernex-core>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
