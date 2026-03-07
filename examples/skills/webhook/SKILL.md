---
name = "webhook"
description = "Send HTTP webhooks and API requests to external services."
requires = ["curl"]
homepage = "https://curl.se/"
trigger = "webhook|api call|http request|post to|send to|notify|trigger workflow"

[toolbox.webhook_post]
description = "Send a POST request with JSON body to a URL."
command = "curl"
args = ["-s", "-X", "POST", "-H", "Content-Type: application/json"]
parameters = { type = "object", properties = { url = { type = "string", description = "Target webhook URL" }, body = { type = "string", description = "JSON body to send" }, headers = { type = "object", description = "Additional headers (optional)" } }, required = ["url", "body"] }

[toolbox.webhook_get]
description = "Send a GET request to a URL."
command = "curl"
args = ["-s"]
parameters = { type = "object", properties = { url = { type = "string", description = "Target URL" }, headers = { type = "object", description = "Additional headers (optional)" } }, required = ["url"] }
---

# Webhook & HTTP Requests

Send HTTP requests to external services, webhooks, and APIs. Essential for
integrating with automation platforms like N8N, Make.com, Zapier, and custom APIs.

## Tools available

- `webhook_post` — Send POST requests with JSON bodies
- `webhook_get` — Send GET requests

## Common integrations

### N8N
```bash
# Trigger an N8N webhook
curl -X POST https://your-n8n.com/webhook/abc123 \
  -H "Content-Type: application/json" \
  -d '{"event": "task_completed", "data": {...}}'
```

### Make.com (Integromat)
```bash
# Trigger a Make scenario
curl -X POST https://hook.make.com/your-webhook-id \
  -H "Content-Type: application/json" \
  -d '{"action": "process", "payload": {...}}'
```

### Zapier
```bash
# Trigger a Zap
curl -X POST https://hooks.zapier.com/hooks/catch/123/abc/ \
  -H "Content-Type: application/json" \
  -d '{"data": "value"}'
```

### Slack
```bash
# Send a Slack notification
curl -X POST https://hooks.slack.com/services/T.../B.../xxx \
  -H "Content-Type: application/json" \
  -d '{"text": "Agent completed task!"}'
```

### Discord
```bash
# Send a Discord message
curl -X POST https://discord.com/api/webhooks/ID/TOKEN \
  -H "Content-Type: application/json" \
  -d '{"content": "Task update from agent"}'
```

## Usage examples

**Notify when a task is done:**
```
Send a webhook to https://hooks.slack.com/... with message "Build completed"
```

**Trigger an automation:**
```
POST to my n8n webhook to start the deployment process
```

**Call an API:**
```
Make an API call to https://api.example.com/data
```

## Security notes

- Never include API keys or tokens in plain text — use environment variables
- Webhook URLs should be kept private
- Consider rate limiting for external API calls
- Use HTTPS for all webhook endpoints
