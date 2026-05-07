# kernex-memory

Persistent memory store for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Backed by SQLite (via `sqlx`), the store records:

- Conversation history and FTS5-indexed message recall
- User facts and project-scoped facts
- Lessons (reward-weighted recall), outcomes, and consolidator state
- Token usage and per-dimension breakdown (input / output / prompt-cache reads / prompt-cache creations)
- Phase checkpoints for multi-stage workflows
- An audit log of permission decisions

The schema is migration-managed; downgrading is not supported. Files are created with mode 0o600 on Unix, and the parent directory is locked to 0o700.

You usually consume memory through [`kernex-runtime`](https://crates.io/crates/kernex-runtime), which wires `Store` into the request pipeline (history → facts → recall → outcome storage). Depend on `kernex-memory` directly when building a side-channel tool that reads or migrates memory state.

## Documentation

- API reference: <https://docs.rs/kernex-memory>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
