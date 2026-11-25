# kernex-skills

Skill loader, parser, and trigger matcher for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Skills are reusable behavior packages with frontmatter metadata, optional MCP servers, and optional toolboxes (script-backed tools). This crate handles:

- Parsing `SKILL.md` frontmatter (TOML or YAML)
- Loading per-skill `mcp.json` and `toolbox.json`
- Resolving CLI tool availability against `$PATH`
- Trigger-keyword matching against incoming messages
- Permission parsing and the load-time checks: `permissions.commands` is an allow-list for toolbox commands, and an MCP server is loaded only when its command is listed there (an empty list starts no server); `permissions.network` gives a toolbox or MCP server network access inside the sandbox (an MCP server declared in the frontmatter can also set `network = true` on its `[mcp.<name>]` entry; `mcp.json` has no such field); `permissions.env` names the only parent variables it receives
- Project-scoped skill installation paths

The runtime calls into this crate to build the per-request context (matched MCP servers, toolboxes, system-prompt fragments). Most callers depend on [`kernex-runtime`](https://crates.io/crates/kernex-runtime) and never touch `kernex-skills` directly; use it standalone when building a custom skill validator or migration tool.

## Documentation

- API reference: <https://docs.rs/kernex-skills>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
