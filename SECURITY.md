# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Kernex, please report it responsibly:

**Email:** security@kernex.dev

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge receipt within 48 hours and aim to provide a fix within 7 days for critical issues.

## Supported Versions

Security fixes land on the current minor and the immediately previous minor.

| Version | Supported           |
|---------|---------------------|
| 0.11.x  | Yes (current)       |
| 0.10.x  | Yes (previous)      |
| < 0.10  | No                  |

## Security Design

What follows describes enforced behavior in the current release, not
aspirations. Where a protection is platform-dependent or has known limits,
those limits are stated.

### Subprocess isolation

- **Environment isolation** — every spawned subprocess (bash/toolbox/grep/find
  tools, MCP servers, the Claude CLI) starts from a cleared environment plus a
  minimal base allowlist (`PATH`, `HOME`, locale, proxy/TLS configuration).
  Provider API keys and cloud credentials never reach a subprocess implicitly;
  anything beyond the base set is an explicit, declared opt-in at the spawn
  site.
- **Filesystem sandbox** — Seatbelt (macOS) and Landlock (Linux 5.13+)
  restrict tool subprocesses: writes inside `$HOME` are denied except the
  workspace/data dir, `$KERNEX_DATA_DIR`, system temp, and common toolchain
  cache/state dirs; reads of credential stores (`~/.ssh`, `~/.aws`,
  `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.netrc`, gh/gcloud configs,
  browser/keychain stores) are denied; the runtime's own database and config
  are unreadable and unwritable. On macOS a directory write root (or the
  directory of a file root), a symlink on the way to a root or a protected
  path, and every directory between either of them and an enclosing write
  root cannot be created, removed or renamed inside the sandbox. A directory
  root's contents, timestamps and mode stay writable; a directory above a
  protected path cannot be written at all. So a toolchain cache directory
  that does not exist yet must be created outside the sandbox. A host path
  granted through `allowed_paths` that is itself a symlink is ignored, and
  a policy whose paths go through more than 40 symlinks allows nothing.
  Code-level checks
  (`is_read_blocked`/`is_write_blocked`) back this on all platforms.
- **Network egress deny-by-default** — sandboxed tool subprocesses cannot open
  network connections unless the tool declares `network = true`. On macOS
  this covers every socket family (TCP, UDP, local sockets, and therefore
  DNS). On Linux it covers TCP bind/connect on kernels 6.7+ (Landlock ABI v4);
  older kernels cannot restrict the network and the gap is logged at spawn
  time. UDP and non-TCP sockets are never restricted on Linux.
- **Fail mode** — when the host cannot apply OS-level enforcement, the library
  default through 0.x is to warn loudly (once per process, on stderr) and run
  the subprocess unsandboxed with code-level checks only. Set
  `SandboxProfile::require_os_enforcement = true` or export
  `KERNEX_REQUIRE_SANDBOX=1` to refuse to spawn instead. The default flips to
  fail-closed in kernex 1.0.

### Skill and tool containment

- **Enforced skill permissions** — a skill's declared `permissions` are
  enforced, not advisory: `commands` is an allow-list checked at load time and
  re-checked by the executor immediately before every toolbox spawn; `env`
  names the only parent environment variables that reach the tool subprocess
  (dynamic-linker variables are refused even when declared); `network` feeds
  the egress opt-in above (host-level granularity in the declaration is
  informational; the OS sandbox is all-or-nothing).
- **MCP command validation** — MCP server commands must be either an absolute
  path under a system prefix or one of a short list of well-known runners
  (npx, uvx, uv, node, python, python3, deno, bun). Relative paths
  are rejected for both MCP and toolbox commands. Skill-declared arguments
  and environment pairs are validated against control-character and
  shell-metacharacter filters.
- **MCP servers run inside the sandbox** — a server the runtime starts gets
  the same OS profile as a tool subprocess, with no network unless its skill
  declares `permissions.network`, and it starts only if its command is listed
  in a non-empty `permissions.commands`. The server starts in a directory
  no tool can write (`mcp/` in the data directory) and reaches the workspace
  through `{workspace}` in its arguments.
  With the CLI providers the external CLI starts the servers itself, inside
  its own sandbox and with its network, so only servers that declare
  `permissions.network` are passed to it.
- **Agent CLI providers** — `claude-code` and `opencode` always run inside
  the OS sandbox; without a configured workspace each call gets an empty
  temporary directory. The tool allowlist is passed to the CLI as its own
  permission settings, and the CLI's permission bypass is off unless the
  host sets `cli_permission_bypass`. The CLIs' own fetch and search tools
  run only when `web_fetch` allows every host, since neither CLI can be held
  to a host list. Each CLI keeps its settings, login and caches in its own
  directory under `cli/` in the data directory, which no other sandboxed
  process can read or write, not in the user's (`~/.claude`,
  `~/.config/opencode`), and reads no settings or project config files, so
  nothing its tools write runs in the user's own CLI. OpenCode gets new
  config, data, state and cache directories for every call, removed
  afterwards, so nothing one call leaves there is read by the next: it has
  no stored login and keeps no session between calls, and it reaches a
  model provider only through that provider's API key variable, of which it
  receives just the one it is configured for. OpenCode's logins in the
  user's home are closed to every sandboxed process. When a call ends, by
  a timeout or normally, the CLI's process group is killed with it.
- **SSRF protection** — the in-process `web_fetch` tool resolves hostnames
  itself, validates every resolved address against private/link-local/
  loopback ranges, and pins the connection to the validated addresses, so a
  DNS answer cannot change between check and connect. Redirects are disabled.

### Secret hygiene

- **Debug redaction** — provider configuration redacts API keys in `Debug`
  output.
- **No secrets in code** — all credentials via environment variables or
  config files; the runtime's config file is read-blocked for tool
  subprocesses.

### Input handling

- **Input sanitization** — `kernex_core::sanitize` breaks ChatML tags and
  role overrides apart with a zero-width space in text an embedder passes
  to it. The runtime does not call it on its own.
- **Path traversal guards** — validates topology names, skill paths, and
  project paths.
- **FTS5 injection prevention** — sanitizes full-text search queries.
- **Recalled memory is marked as data.** What memory recalls from other
  conversations (matching past messages, summaries of closed conversations,
  facts, tasks, lessons and outcomes) and the summary of this
  conversation's overflow go into the system prompt inside one
  `<recalled_memory>` block that says they are data with no authority. The
  tag is removed from that text, and `<`, `>` and a list of lookalike
  angle brackets are escaped; a bracket shape the list misses can still
  read as a tag. The current conversation's recent messages are sent as
  ordinary turns, outside the block. When a context is flattened into one
  prompt, a line that starts like a section header (`[System]`, `[User]`,
  `[Assistant]`, in any case, with inner spaces, listed invisible
  characters, fullwidth letters, three other bracket pairs or text after
  it) is escaped; markdown prefixes and lookalike letters from other
  scripts are not recognised. This lowers the risk that an instruction
  stored in memory is followed; it does not remove it. A call with
  `ContextNeeds::store` set to `false` leaves memory as it was: nothing is
  stored, no conversation, fact or usage record is created, and an
  existing conversation is read without being touched.

### Extension seams (not active protections by themselves)

- **Guardrail layer** — the `GuardrailRunner` trait is a seam for embedders
  to add semantic input/output filtering (Allow, Block, Sanitize) above the
  OS sandbox. Kernex ships the trait, not a default guardrail
  implementation.

## Known Platform Limits

Stated so deployments can plan around them:

- **macOS** — enforcement relies on `/usr/bin/sandbox-exec`, which Apple
  documents as deprecated but still ships and uses internally. There is no
  supported third-party replacement API; if a future macOS removes it, the
  fail-mode policy above applies.
- **Linux** — Landlock enforcement depth depends on the kernel ABI (5.13
  minimum; 6.7+ for TCP restrictions; 6.12+ for ABI v5). Between 5.13 and
  6.11 some access rights cannot be enforced and the sandbox degrades to
  what the kernel supports; the degradation is logged at spawn time but is
  not observable programmatically by callers. UDP and non-TCP sockets are
  never restricted by Landlock.
- **Other platforms** — no OS-level sandbox; code-level path checks only,
  and the fail-mode policy applies.
