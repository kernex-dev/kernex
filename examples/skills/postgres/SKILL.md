---
name = "postgres"
description = "Read-only PostgreSQL access with schema inspection via MCP."
requires = ["npx"]
homepage = "https://github.com/modelcontextprotocol/servers/tree/main/src/postgres"
trigger = "postgres|database|sql|query|table|schema"

[mcp.postgres]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"]
---

# PostgreSQL

Read-only access to PostgreSQL databases through MCP. Useful for agents that
need to inspect data, run queries, or understand database schemas.

## Tools available

- `query` — Execute read-only SQL queries
- `list_tables` — List all tables in the database
- `describe_table` — Get column definitions and types

## Configuration

Replace the connection string in `args` with your database URL:

```toml
[mcp.postgres]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres", "postgresql://user:pass@host:5432/dbname"]
```

## Security

The server enforces read-only access. Write operations are rejected.
