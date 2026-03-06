---
name = "git"
description = "Git repository operations (log, diff, blame, branches) via MCP."
requires = ["npx", "git"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/git"
trigger = "git|commit|branch|diff|blame|log"

[mcp.git]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-git"]
---

# Git

Read and manipulate Git repositories through MCP. Useful for agents that
need to inspect code history, review changes, or manage branches.

## Tools available

- `git_log` — View commit history with optional filtering
- `git_diff` — Show changes between commits, branches, or working tree
- `git_blame` — Show line-by-line authorship
- `git_status` — Show working tree status
- `git_branch` — List or create branches
- `git_checkout` — Switch branches or restore files
- `git_commit` — Create commits with staged changes

## Notes

The server operates on the repository in the current working directory.
Set the agent's workspace path to the target repo.
