# kernex-providers

LLM provider implementations and the shared agentic tool-execution loop for the [Kernex](https://github.com/kernex-dev/kernex) AI agent runtime.

Ships 11 production providers behind the unified `Provider` trait from `kernex-core`:

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
| Claude Code (CLI) | OAuth | no (subprocess) | yes |

AWS Bedrock is also available behind the `bedrock` Cargo feature.

The crate also exposes the shared 7-tool builtin executor (Bash, Read, Write, Edit, Grep, Glob, WebFetch) used by every provider's tool-call loop, with sandbox enforcement via [`kernex-sandbox`](https://crates.io/crates/kernex-sandbox).

You usually consume providers via [`kernex-runtime`](https://crates.io/crates/kernex-runtime)'s factory. Depend on `kernex-providers` directly when wiring a non-runtime executor or when you only need provider clients.

## Documentation

- API reference: <https://docs.rs/kernex-providers>
- Project overview: <https://github.com/kernex-dev/kernex>

## License

Apache-2.0 OR MIT.
