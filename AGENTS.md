# AGENTS.md

This repository follows the three-layer Claude documentation model. Project
instructions live in [`CLAUDE.md`](CLAUDE.md) (Layer 2) and on-demand context
lives in [`.claude/docs/`](.claude/docs/) (Layer 3).

If your agent (Codex, Cursor, Qwen-Coder, Gemini, Aider, etc.) reads
`AGENTS.md` rather than `CLAUDE.md`, treat the two files as equivalent: the
canonical guidance is in `CLAUDE.md`. There is no separate AGENTS-only
contract.

## Quick links

- Project rules and dev commands: [`CLAUDE.md`](CLAUDE.md)
- Static reference (architecture, deployment, schemas): [`.claude/docs/CONTEXT.md`](.claude/docs/CONTEXT.md)
- Append-only learnings (decisions, gotchas, audit punch-lists): [`.claude/docs/LEARNINGS.md`](.claude/docs/LEARNINGS.md)
- Security policy: [`SECURITY.md`](SECURITY.md)
- Contributing guide: [`CONTRIBUTING.md`](CONTRIBUTING.md)

## Pre-commit gate

Every change must pass:

```
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --all -- --check
cargo deny check
```

CI runs the same set on Linux and macOS. See
[`.github/workflows/ci.yml`](.github/workflows/ci.yml).

## Authorship

Do not add `Co-Authored-By` trailers or rewrite AI-tool authorship via
`.mailmap`. Commits are signed by the human who reviewed and pushed them; the
agent's involvement is a working detail, not a contributor.
