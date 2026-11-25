# kernex-memory

Persistent memory store for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Backed by SQLite (via `sqlx`), the store records:

- Conversation history and FTS5-indexed message recall
- User facts and project-scoped facts
- Lessons (reward-weighted recall), outcomes, and consolidator state
- Token usage and per-dimension breakdown (input / output / prompt-cache reads / prompt-cache creations)
- Phase checkpoints for multi-stage workflows
- An audit log of permission decisions

When `Store::build_context` assembles a prompt, everything recalled from memory (past messages, summaries, facts, tasks, lessons, outcomes, the overflow summary) goes into one `<recalled_memory>` block in the system prompt that marks it as data with no authority, with the tag and angle brackets escaped inside it. This lowers the risk that an instruction stored in memory is followed; it does not remove it. With `ContextNeeds::store` set to `false` nothing is written. A context built without a project reads the `"default"` conversation, where exchanges without a project are stored.

The schema is migration-managed; downgrading is not supported. Files are created with mode 0o600 on Unix, and the parent directory is locked to 0o700.

You usually consume memory through [`kernex-runtime`](https://crates.io/crates/kernex-runtime), which wires `Store` into the request pipeline (history → facts → recall → outcome storage). Depend on `kernex-memory` directly when building a side-channel tool that reads or migrates memory state.

## Documentation

- API reference: <https://docs.rs/kernex-memory>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
