# Reference Skills

Ready-to-use skill definitions for Kernex agents. Each skill is a `SKILL.md` file
with TOML frontmatter that declares triggers and MCP servers.

## Available skills

| Skill | MCP Server | Description |
|-------|-----------|-------------|
| [filesystem](filesystem/) | `@modelcontextprotocol/server-filesystem` | Secure file operations |
| [git](git/) | `@modelcontextprotocol/server-git` | Git repository operations |
| [playwright](playwright/) | `@playwright/mcp` | Browser automation (Microsoft) |
| [github](github/) | `@modelcontextprotocol/server-github` | GitHub API access |
| [postgres](postgres/) | `@modelcontextprotocol/server-postgres` | PostgreSQL read-only access |
| [sqlite](sqlite/) | `@modelcontextprotocol/server-sqlite` | SQLite read/write access |
| [brave-search](brave-search/) | `@modelcontextprotocol/server-brave-search` | Web search via Brave API |

## Usage

Copy the skills you need into your agent's data directory:

```bash
# Copy a single skill
cp -r examples/skills/playwright ~/.kernex/skills/

# Copy all skills
cp -r examples/skills/*/ ~/.kernex/skills/
```

Then load them in your Kernex runtime:

```rust
use kernex_skills::load_skills;

let skills = load_skills("~/.kernex");
```

## Creating your own

Use the [`_template/`](_template/) directory as a starting point:

```bash
cp -r examples/skills/_template ~/.kernex/skills/my-skill
# Edit ~/.kernex/skills/my-skill/SKILL.md
```

### SKILL.md format

```toml
---
name = "my-skill"
description = "What this skill does."
requires = ["npx"]                        # CLIs that must exist in $PATH
trigger = "keyword1|keyword2"             # Pipe-separated trigger words
homepage = "https://example.com"          # Optional

[mcp.server-name]                         # MCP server declaration
command = "npx"
args = ["-y", "@scope/package"]
---

# Instructions

Markdown body injected into the agent's system prompt when the skill is active.
```

### Key concepts

- **`requires`** — Kernex checks these binaries exist in `$PATH` before activating the skill
- **`trigger`** — Case-insensitive keyword matching against user messages
- **`[mcp.*]`** — MCP servers are started dynamically when the skill triggers
- **Body** — The markdown below the frontmatter becomes part of the system prompt

### Skills.sh compatibility

Kernex supports both TOML and YAML frontmatter, compatible with the
[Skills.sh](https://skills.sh) standard.
