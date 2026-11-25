# kernex-providers

LLM provider implementations and the shared agentic tool-execution loop for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Ships 12 production providers behind the unified `Provider` trait from `kernex-core`:

| Provider | Auth | Streaming | Tools |
|----------|------|-----------|-------|
| Anthropic | API key | yes | yes |
| OpenAI | API key | yes | yes |
| Gemini (Google) | API key | yes | yes |
| Ollama | local | yes | yes |
| OpenRouter | API key | yes | yes |
| Groq | API key | yes | yes |
| Mistral | API key | yes | yes |
| DeepSeek | API key | yes | yes |
| Fireworks | API key | yes | yes |
| xAI | API key | yes | yes |
| Claude Code (CLI) | OAuth token or `ANTHROPIC_API_KEY` | no (subprocess) | yes |
| OpenCode (CLI) | API key of the model's provider | no (subprocess) | yes |

AWS Bedrock is also available behind the `bedrock` Cargo feature.

The crate also exposes the shared 7-tool builtin executor (Bash, Read, Write, Edit, Grep, Glob, WebFetch) used by every provider's tool-call loop, with sandbox enforcement via [`kernex-sandbox`](https://crates.io/crates/kernex-sandbox):

- Bash, toolbox tools and MCP servers run inside the OS sandbox. An MCP server starts only when its command is listed in the skill's `permissions.commands`, and gets the network only when the skill grants it (`permissions.network`, or `network = true` on its `[mcp.<name>]` frontmatter entry) or the host profile already allows it.
- Read, Write, Edit, Grep and Glob reach the workspace and the profile's `allowed_paths`, plus `read_paths` and the runtime's `skills/` and `projects/` for reads, and nothing else.
- WebFetch is off by default. Set `ProviderConfig::web_fetch` to a `WebFetchPolicy` host list or `AllowAll` to offer it.
- `allowed_tools` offers exactly the tools it names, MCP and toolbox tools included. Permission rules match one value per call (the command, the resolved path or the URL).
- Set `ProviderConfig::data_dir` (and `config_path`, `db_path` when they live elsewhere) to the directories the runtime really uses, so tool calls cannot change them.

The CLI providers always run inside the OS sandbox; without a workspace each call gets an empty temporary directory. Each CLI keeps its state in `cli/` under the data directory, not in the user's own config, and its permission bypass is off unless `cli_permission_bypass` is set. Claude Code needs an OAuth token, `ANTHROPIC_API_KEY`, or a login made with `CLAUDE_CONFIG_DIR` set to `ClaudeCodeProvider::cli_config_dir()`. OpenCode receives only the API key variable of its model's provider and keeps no session between calls.

You usually consume providers via [`kernex-runtime`](https://crates.io/crates/kernex-runtime)'s factory. Depend on `kernex-providers` directly when wiring a non-runtime executor or when you only need provider clients.

## Documentation

- API reference: <https://docs.rs/kernex-providers>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
