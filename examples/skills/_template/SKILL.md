---
name = "my-skill"
description = "Short description of what this skill does."
requires = ["npx"]
homepage = "https://example.com"
trigger = "keyword1|keyword2|keyword3"

[mcp.my-server]
command = "npx"
args = ["-y", "@scope/mcp-server-name"]
---

# My Skill

Explain what this skill enables the agent to do.

## Tools available

- `tool_name` — What this tool does

## Setup

Any required environment variables or prerequisites:

```bash
export API_KEY="..."
```

## Notes

Additional context, limitations, or configuration tips.
