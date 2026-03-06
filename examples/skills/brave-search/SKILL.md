---
name = "brave-search"
description = "Web and local search via Brave Search API and MCP."
requires = ["npx"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/brave-search"
trigger = "search|web search|find online|look up|google"

[mcp.brave-search]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-brave-search"]
---

# Brave Search

Web and local search powered by the Brave Search API through MCP.
Requires a `BRAVE_API_KEY` environment variable.

## Tools available

- `brave_web_search` — Search the web with pagination support
- `brave_local_search` — Search for local businesses and places

## Setup

1. Get a free API key at https://brave.com/search/api/
2. Set it as an environment variable:

```bash
export BRAVE_API_KEY="BSA..."
```

## Use cases

- Research and fact-checking
- Finding current information (news, events)
- Local business discovery
- Supplementing agent knowledge with real-time data
