# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Kernex, please report it responsibly:

**Email:** security@kernex.dev

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge receipt within 48 hours and aim to provide a fix within 7 days for critical issues.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Security Design

Kernex is built with security as a core principle:

- **OS-level sandboxing** — Seatbelt (macOS) and Landlock (Linux) restrict file system access for AI tool execution
- **Code-level path protection** — blocks reads/writes to config files, data directories, and system paths
- **Input sanitization** — neutralizes prompt injection attempts (ChatML tags, role overrides, zero-width characters)
- **MCP command validation** — rejects shell metacharacters in MCP server commands
- **Path traversal guards** — validates topology names, skill paths, and project paths
- **FTS5 injection prevention** — sanitizes full-text search queries
- **No secrets in code** — all credentials via environment variables or config files
