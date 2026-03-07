# Roadmap

Technical roadmap for Kernex development. Items are organized by priority and complexity.

## Current State (v0.3)

- 7 composable crates (core, sandbox, providers, memory, skills, pipelines, runtime)
- 6 AI providers with dynamic instantiation via `ProviderFactory`
- 9 skills (filesystem, git, playwright, github, postgres, sqlite, brave-search, pdf, webhook)
- OS-level sandboxing (Seatbelt + Landlock)
- SQLite-backed memory with FTS5 search
- TOML-defined multi-agent pipelines
- 375+ tests across all crates

## Short Term (v0.4)

### Observability

| Feature | Status | Description |
|---------|--------|-------------|
| Public tracing API | Planned | Expose `tracing` spans for external subscribers |
| Token tracking | Planned | Track input/output tokens per request and conversation |
| Request logging | Exists | Audit log in `kernex-memory`, needs documentation |
| OpenTelemetry export | Planned | Optional OTLP exporter for traces and metrics |

### Developer Experience

| Feature | Status | Description |
|---------|--------|-------------|
| Error messages | Ongoing | Improve error context with `thiserror` |
| Docs.rs examples | Planned | Runnable examples in doc comments |
| Migration guide | Planned | v0.2 → v0.3 breaking changes |

## Medium Term (v0.5)

### Dynamic Pipelines

Current state: Topologies are defined statically in `TOPOLOGY.toml`.

Planned improvements:

```
planner
   │
   ├── [condition: needs_research]
   │   └── research
   │
   └── [condition: needs_code]
       └── coding
              │
            review
```

| Feature | Status | Description |
|---------|--------|-------------|
| Conditional phases | Planned | Branch based on runtime conditions |
| Dynamic agent spawn | Planned | Create agents based on planner output |
| Phase callbacks | Planned | Hooks for pre/post phase execution |

### Memory Enhancements

| Feature | Status | Description |
|---------|--------|-------------|
| Vector embeddings | Research | Optional semantic search via embeddings |
| Memory compaction | Planned | Summarize old conversations to save space |
| Cross-project recall | Planned | Share lessons across projects |

## Long Term (v1.0)

### Ecosystem

| Feature | Status | Description |
|---------|--------|-------------|
| Skill registry | Research | Central registry for community skills |
| Provider plugins | Research | Dynamic provider loading without recompile |
| Python bindings | Research | PyO3 bindings for Python ecosystem |

### Production Readiness

| Feature | Status | Description |
|---------|--------|-------------|
| Benchmarks | Planned | Criterion benchmarks for critical paths |
| Stress testing | Planned | Long-running agent stability tests |
| Security audit | Planned | Third-party security review |

## Non-Goals

These are explicitly out of scope:

- **GUI/Web interface** — Kernex is a runtime, not an application
- **Hosted service** — Self-hosted only, no SaaS offering
- **LangChain compatibility** — Different design philosophy, no adapter layer

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to contribute. Roadmap items marked "Planned" are open for contribution.

## Versioning

- **0.x** — Breaking changes allowed between minor versions
- **1.0** — Stable API, breaking changes only in major versions

Current focus: Getting to 1.0 with stable Provider, Store, and Runtime traits.
