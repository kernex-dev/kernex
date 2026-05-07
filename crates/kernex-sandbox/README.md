# kernex-sandbox

OS-level sandbox primitives for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Wraps platform-native isolation so tool execution and skill subprocesses run with the smallest workable privilege set:

- **macOS**: Seatbelt (`sandbox-exec`) profiles generated as SBPL strings
- **Linux**: Landlock LSM (kernel 6.x+ for full enforcement, partial on older kernels)
- **Other platforms**: a no-op profile that compiles cleanly so cross-platform code does not fork

The crate exposes `SandboxProfile` (configurable read/write/exec rules) and a `pre_exec` helper that applies the active profile inside `tokio::process::Command`.

You usually consume this through [`kernex-runtime`](https://crates.io/crates/kernex-runtime); use it directly only when building a custom executor.

## Documentation

- API reference: <https://docs.rs/kernex-sandbox>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
