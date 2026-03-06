---
name = "filesystem"
description = "Secure file operations (read, write, search, list) via MCP."
requires = ["npx"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem"
trigger = "file|read file|write file|list files|directory|folder"

[mcp.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
---

# Filesystem

Provides secure file system access through MCP. The server restricts operations
to the configured directory (`.` by default — adjust the last arg to your workspace).

## Tools available

- `read_file` — Read complete file contents
- `write_file` — Create or overwrite files
- `edit_file` — Apply surgical edits to existing files
- `list_directory` — List directory contents
- `search_files` — Recursively search for files matching a pattern
- `get_file_info` — Get file metadata (size, timestamps, permissions)
- `list_allowed_directories` — Show which directories the server can access

## Configuration

Change the last argument in `args` to restrict access to a specific directory:

```toml
[mcp.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]
```
