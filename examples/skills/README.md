# Reference Skills

Ready-to-use skill definitions for Kernex agents. Each skill is a `SKILL.md` file
with TOML frontmatter that declares triggers and MCP servers.

## Available skills

| Skill | MCP Server / CLI | Description |
|-------|-----------------|-------------|
| [filesystem](filesystem/) | `@modelcontextprotocol/server-filesystem` | Secure file operations |
| [git](git/) | `@modelcontextprotocol/server-git` | Git repository operations |
| [playwright](playwright/) | `@playwright/mcp` | Browser automation (Microsoft) |
| [github](github/) | `@modelcontextprotocol/server-github` | GitHub API access |
| [postgres](postgres/) | `@modelcontextprotocol/server-postgres` | PostgreSQL read-only access |
| [sqlite](sqlite/) | `@modelcontextprotocol/server-sqlite` | SQLite read/write access |
| [brave-search](brave-search/) | `@modelcontextprotocol/server-brave-search` | Web search via Brave API |
| [pdf](pdf/) | `pdftotext` (CLI) | Read and extract text from PDFs |
| [webhook](webhook/) | `curl` (CLI) | Send HTTP webhooks to external services |

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

## Create Your First Skill (5 minutes)

### Step 1: Copy the Template

```bash
mkdir -p ~/.kernex/skills
cp -r examples/skills/_template ~/.kernex/skills/my-skill
```

### Step 2: Edit SKILL.md

Open `~/.kernex/skills/my-skill/SKILL.md` and customize:

```toml
---
name = "my-skill"
description = "What this skill does"
requires = ["npx"]  # CLI tools needed
trigger = "keyword1|keyword2|search term"

[mcp.my-server]
command = "npx"
args = ["-y", "@scope/mcp-server-name"]
---

# My Skill

Instructions for the AI about how to use this skill.

## Tools Available

- `tool_name` — What it does
```

### Step 3: Test Your Skill

```bash
cargo run --example skill_loader
# Should show: my-skill — What this skill does [ready]
```

### Step 4: Use in kx

```bash
kx "keyword1 something"
# Kernex detects trigger and activates your skill
```

## Skill Activation by Trigger

When you mention these keywords, the corresponding skill activates:

| Say This | Activates | Example |
|----------|-----------|---------|
| "read the file", "list directory" | filesystem | "read the config file" |
| "git log", "commit changes" | git | "show recent commits" |
| "browse website", "screenshot" | playwright | "browse docs.rs" |
| "check PR", "github issue" | github | "list open issues" |
| "search for" | brave-search | "search for rust async" |

## MCP Server Requirements

Most skills use MCP servers via npx. Ensure you have:

1. **Node.js 18+** — [nodejs.org](https://nodejs.org)
2. **npx** — Comes with Node.js

Some skills need API keys:
- **brave-search**: Set `BRAVE_SEARCH_API_KEY`
- **github**: Set `GITHUB_TOKEN` (for private repos)

## Troubleshooting

### "Skill not loading"
- Check `~/.kernex/skills/` exists
- Verify SKILL.md has valid TOML frontmatter
- Run `cargo run --example skill_loader` to debug

### "MCP server failed"
- Ensure `npx` is installed: `npx --version`
- Check the MCP package exists: `npx -y @package/name --help`
- Look for missing env vars (API keys)

### "Trigger not matching"
- Triggers are case-insensitive
- Use `|` to separate multiple triggers
- Be specific: "read file" matches, "r" doesn't
