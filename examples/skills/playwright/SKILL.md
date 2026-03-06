---
name = "playwright"
description = "Browser automation via Playwright MCP — navigate, click, extract content."
requires = ["npx"]
homepage = "https://github.com/microsoft/playwright-mcp"
trigger = "browse|website|click|scrape|web page|navigate|screenshot"

[mcp.playwright]
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
---

# Playwright

Browser automation powered by Microsoft's official Playwright MCP server.
Uses accessibility snapshots for fast, reliable interaction without screenshots.

## Tools available

- `browser_navigate` — Navigate to a URL
- `browser_click` — Click an element by accessible name or role
- `browser_type` — Type text into an input field
- `browser_snapshot` — Get the current page accessibility tree
- `browser_screenshot` — Capture a screenshot (when visual context needed)
- `browser_evaluate` — Execute JavaScript in the page context

## Modes

- `--headless` (default) — No visible browser window
- Remove `--headless` for debugging with a visible browser

## Use cases

- Web scraping and data extraction
- Form filling and submission
- E2E testing automation
- Research and content gathering
