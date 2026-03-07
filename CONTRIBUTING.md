# Contributing to Kernex

Thank you for your interest in contributing to Kernex. This document outlines the process for contributing to the project.

## Before You Start

This guide provides the essential information for contributors. For architecture details, see the [README](README.md).

## Development Setup

1. Clone the repository:

```bash
git clone https://github.com/kernex-dev/kernex.git
cd kernex
```

2. Build all crates:

```bash
cargo build --workspace
```

3. Run the test suite:

```bash
cargo test --workspace
```

## Pre-Commit Checklist

All checks must pass before committing:

| Step | Command |
|------|---------|
| 1 | `cargo build --workspace` |
| 2 | `cargo audit && cargo deny check` |
| 3 | `cargo clippy --workspace -- -D warnings` |
| 4 | `cargo test --workspace` |
| 5 | `cargo fmt --check` |

Only commit after all five steps pass.

## Commit Style

Use [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` — new feature
- `fix:` — bug fix
- `refactor:` — code restructuring without behavior change
- `docs:` — documentation only
- `test:` — adding or updating tests
- `chore:` — maintenance tasks (deps, config, CI)

Keep commits atomic: one logical change per commit.

## Pull Request Process

1. Fork the repository and create a feature branch:

```bash
git checkout -b feat/my-feature
```

2. Make your changes and run the full pre-commit gate.

3. Push to your fork and open a Pull Request against `main`.

4. Ensure CI passes. Address any review feedback.

## Code Standards Summary

- **No `unwrap()` or `expect()`** in production code. Use `?` and proper error types.
- **Error handling:** `thiserror` for library crates, `anyhow` only at binary boundaries.
- **Doc comments:** Every public type and function must have documentation.
- **File size:** No file exceeds 500 lines (excluding tests).
- **Logging:** Use `tracing`, not `println!`.
- **Async:** All I/O is async via Tokio.

For architecture details, see the [README](README.md).

## License

By contributing, you agree that your contributions will be licensed under the same terms as the project: Apache-2.0 OR MIT.
