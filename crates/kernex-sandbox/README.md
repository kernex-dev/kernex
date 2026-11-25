# kernex-sandbox

OS-level sandbox primitives for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Wraps platform-native isolation so tool execution and skill subprocesses run with the smallest workable privilege set. One path policy, built from a `SandboxScope` (workspace, runtime data directory, optional config and database paths) and a `SandboxProfile`, feeds both the OS profile and the code-level checks:

- Writes only under the workspace, the system temp directories, the per-user toolchain cache directories (their `bin` directories and startup files stay read-only) and `allowed_paths`. The runtime data directory is not a write root: its database, config, `skills/`, `projects/` and `mcp/` stay unwritable even when it sits inside one.
- Reads everywhere except credential stores, agent CLI state, shell history, the memory database, the config file and `blocked_paths`.
- No network unless `allow_network` is set.

Platform back ends:

- **macOS**: a Seatbelt (`sandbox-exec`) profile that denies by default. No keychain access unless `allow_keychain` is set, no local sockets, no pseudo-terminals.
- **Linux**: Landlock LSM. 5.13 is the minimum, 6.2 (ABI 3) is the first kernel that counts as enforcing, TCP is restricted from 6.7 and abstract Unix sockets and signals are scoped from 6.12. Older kernels apply the rights they support. UDP and pathname Unix sockets are never restricted.
- **Other platforms**: no OS-level sandbox; code-level path checks only.

When the host cannot enforce the policy, `protected_command` warns once and runs the command unsandboxed. Set `SandboxProfile::require_os_enforcement` or `KERNEX_REQUIRE_SANDBOX=1` and call `try_protected_command` to refuse instead; that becomes the default in 1.0. For code that runs inside the host process, `tool_read_path`, `tool_write_path` and `tool_search_root` confine a file tool to the workspace, `allowed_paths` and `read_paths`.

The full list of protections and platform limits is in [SECURITY.md](https://github.com/kernex-dev/kernex/blob/main/SECURITY.md).

You usually consume this through [`kernex-runtime`](https://crates.io/crates/kernex-runtime); use it directly only when building a custom executor.

## Documentation

- API reference: <https://docs.rs/kernex-sandbox>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
