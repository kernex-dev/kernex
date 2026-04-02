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
| 0.4.x   | Yes       |
| 0.3.x   | No        |
| < 0.3   | No        |

## Security Design

Kernex is built with security as a core principle:

- **OS-level sandboxing** — Seatbelt (macOS) and Landlock (Linux, kernel 5.13+ minimum, 6.12+ for full ABI::V5 enforcement) restrict file system access for AI tool execution; kernels between 5.13 and 6.11 apply best-effort protection using only the rights they support; code-level enforcement (`is_read_blocked`/`is_write_blocked`) covers all platforms regardless of kernel version
- **Code-level path protection** — blocks reads/writes to config files, data directories, and system paths
- **Guardrail layer** — `GuardrailRunner` trait provides semantic input/output filtering (Allow, Block, Sanitize) at the pipeline layer, above the OS sandbox, for prompt injection and policy enforcement
- **Input sanitization** — neutralizes prompt injection attempts (ChatML tags, role overrides, zero-width characters)
- **MCP command validation** — rejects shell metacharacters in MCP server commands
- **Path traversal guards** — validates topology names, skill paths, and project paths
- **FTS5 injection prevention** — sanitizes full-text search queries
- **No secrets in code** — all credentials via environment variables or config files
