# kernex-pipelines

Multi-agent topology execution for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Provides declarative pipelines that chain multiple agents, route outputs between them, and enforce per-step gates. Topologies are described as TOML and executed against any `Provider` implementation.

Used to build patterns like:

- **2-phase analyst → synthesizer** (a planner reviews a problem, a builder turns the plan into output)
- **N-way reality-check fan-out** (one agent produces, several review, a reducer aggregates)
- **Sequential evaluators** with a final ship/reject gate

You usually consume pipelines through [`kernex-runtime`](https://crates.io/crates/kernex-runtime), which loads topologies from disk and binds them to a runtime instance.

## Documentation

- API reference: <https://docs.rs/kernex-pipelines>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
