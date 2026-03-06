---
name = "sqlite"
description = "SQLite database access with read/write and schema inspection via MCP."
requires = ["npx"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/sqlite"
trigger = "sqlite|database|sql|query|table"

[mcp.sqlite]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-sqlite", "./data.db"]
---

# SQLite

Full access to SQLite databases through MCP. Unlike the PostgreSQL server,
this one supports both reads and writes.

## Tools available

- `read_query` — Execute SELECT queries
- `write_query` — Execute INSERT, UPDATE, DELETE statements
- `create_table` — Create new tables
- `list_tables` — List all tables
- `describe_table` — Get table schema
- `append_insight` — Store analysis notes in a memo table

## Configuration

Change the last argument in `args` to point to your database file:

```toml
[mcp.sqlite]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-sqlite", "/path/to/database.db"]
```
