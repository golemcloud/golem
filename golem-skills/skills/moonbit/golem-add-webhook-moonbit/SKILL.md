---
name: golem-add-webhook-moonbit
description: "Using webhooks in a MoonBit Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a MoonBit Golem Agent

## Overview

Golem webhooks let an agent generate a public URL that, when POSTed to by an external system, delivers the request body to the agent. A webhook is backed by a Golem promise. A durable agent can suspend after the configured idle delay while waiting for the callback; persisted state and host metadata still consume storage.

This is useful for:
- Integrating with webhook-driven APIs (payment gateways, CI/CD, GitHub, Stripe, etc.)
- Receiving asynchronous callbacks from external services
- Building event-driven workflows where an external system notifies the agent

### Prerequisites

The agent type **must** be deployed via an HTTP API mount (`#derive.mount("/...")` on the agent struct and an `httpApi` deployment in `golem.yaml`). Without a mount, webhooks cannot be created.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-moonbit` | Setting up the HTTP mount and endpoint annotations required before using webhooks |
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml`, including `subdomain` versus `domain` |
| `golem-wait-for-external-input-moonbit` | Lower-level promise API if you need more control than webhooks provide |

## API

All functions are in the `@webhook` package of the Golem MoonBit SDK:

| Function / Type | Description |
|---|---|
| `@webhook.create()` | Creates a webhook (promise + public URL) and returns a `WebhookHandler` |
| `WebhookHandler::url(self)` | Returns the public URL to share with external systems |
| `WebhookHandler::wait(self)` | Async: waits for the webhook payload and returns `WebhookRequestPayload` |
| `WebhookRequestPayload::text(self)` | Decodes the POST body as a UTF-8 string |
| `WebhookRequestPayload::bytes(self)` | Returns the raw POST body as `Bytes` |

## Webhook URL Structure

Webhook URLs have the form:

```
https://<domain>/<prefix>/<suffix>/<id>
```

- **`<domain>`** — the domain where the HTTP API is deployed
- **`<prefix>`** — defaults to `/webhooks`, customizable via `webhookUrl` in the `httpApi` deployment section of `golem.yaml`:
  ```yaml
  httpApi:
    deployments:
      local:
      - subdomain: my-app  # resolves to my-app.localhost:9006 by default
        webhookUrl: "/my-custom-webhooks/"
        agents:
          OrderAgent: {}
  ```
- **`<suffix>`** — defaults to the agent type name in `kebab-case` (e.g., `OrderAgent` → `order-agent`), customizable via `#derive.mount_webhook`
- **`<id>`** — a unique identifier for the specific webhook instance

## Webhook Suffix

You can configure a webhook suffix using `#derive.mount_webhook("/path")` on the agent struct to override the default kebab-case agent name in the webhook URL:

```moonbit
#derive.agent
#derive.mount("/api/orders/{id}")
#derive.mount_webhook("/workflow-hooks")
struct OrderAgent {
  id : String
}
```

Webhook suffixes accept literal segments and system variables such as `{agent-type}`, not constructor variables such as `{name}`:

```moonbit
#derive.agent
#derive.mount("/api/events/{name}")
#derive.mount_webhook("/{agent-type}/callbacks")
struct EventAgent {
  name : String
}
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback

Run the waiting fragments below inside an `async fn`. Any helper or method that calls them must also be async; MoonBit has no `await` keyword.

```moonbit
let webhook = @webhook.create()
let url = webhook.url()

// Share `url` with an external service (e.g., register it as a callback URL)
// Waiting can suspend the durable agent after the suspension policy delay.

let payload = webhook.wait()
```

### 2. Decode the Payload as Text

```moonbit
let webhook = @webhook.create()
// ... share webhook.url() ...
let payload = webhook.wait()
let body = payload.text()
```

### 3. Use Raw Bytes

```moonbit
let webhook = @webhook.create()
// ... share webhook.url() ...
let payload = webhook.wait()
let raw : Bytes = payload.bytes()
```

## Complete Example

```moonbit
///|
#derive.agent
#derive.mount("/integrations/{name}")
#derive.mount_auth(false)
struct IntegrationAgent {
  name : String
  mut last_event : String
  mut pending : @webhook.WebhookHandler?
}

///|
fn IntegrationAgent::new(name : String) -> IntegrationAgent {
  { name, last_event: "", pending: None }
}

///|
/// Creates a webhook and publishes its URL before waiting in a separate invocation
#derive.endpoint(post="/register")
pub fn IntegrationAgent::register(self : Self) -> String {
  let webhook = @webhook.create()
  self.pending = Some(webhook)
  webhook.url()
}

///|
#derive.endpoint(post="/wait")
pub async fn IntegrationAgent::wait(self : Self) -> String {
  let webhook = match self.pending {
    Some(webhook) => webhook
    None => return "Register a webhook first"
  }
  let payload = webhook.wait()
  let body = payload.text()
  self.pending = None
  self.last_event = body
  self.last_event
}

///|
/// Returns the last received webhook event
#derive.endpoint(get="/last-event")
pub fn IntegrationAgent::get_last_event(self : Self) -> String {
  self.last_event
}
```

## Key Constraints

- The agent **must** have an HTTP mount (`#derive.mount("/...")`) and be deployed via `httpApi` in `golem.yaml`
- The first POST completes the promise once. The URL is not invalidated: duplicate POSTs return 204 without replacing the original payload
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler::wait()` is async; a pending wait can suspend a durable agent after the suspension policy delay
- The agent survives failures, restarts, and updates while waiting
- **Never edit generated files** — `golem_reexports.mbt`, `golem_agents.mbt`, and `golem_derive.mbt` are auto-generated by `golem build`
