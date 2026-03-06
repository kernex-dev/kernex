---
name = "github"
description = "GitHub API access — repos, issues, PRs, code search via MCP."
requires = ["npx"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/github"
trigger = "github|issue|pull request|PR|repo|repository"

[mcp.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
---

# GitHub

Interact with the GitHub API through MCP. Requires a `GITHUB_PERSONAL_ACCESS_TOKEN`
environment variable.

## Tools available

- `search_repositories` — Search for repos by query
- `search_code` — Search code across GitHub
- `get_file_contents` — Read file contents from a repo
- `create_issue` — Create a new issue
- `list_issues` — List issues with filtering
- `create_pull_request` — Open a PR
- `list_pull_requests` — List PRs with filtering
- `add_issue_comment` — Comment on an issue or PR

## Setup

Set the token as an environment variable before starting the agent:

```bash
export GITHUB_PERSONAL_ACCESS_TOKEN="ghp_..."
```
