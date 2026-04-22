---
name: golem-add-webhook-moonbit
description: "Using webhooks in a MoonBit Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a MoonBit Golem Agent

## Overview

Golem webhooks let an agent generate a temporary public URL that, when POSTed to by an external system, delivers the request body to the agent. Under the hood, a webhook is backed by a Golem promise — the agent is durably suspended while waiting for the callback, consuming no resources.

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
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml` |
| `golem-wait-for-external-input-moonbit` | Lower-level promise API if you need more control than webhooks provide |

## API

All functions are in the `@webhook` package of the Golem MoonBit SDK:

| Function / Type | Description |
|---|---|
| `@webhook.create()` | Creates a webhook (promise + public URL) and returns a `WebhookHandler` |
| `WebhookHandler::url(self)` | Returns the public URL to share with external systems |
| `WebhookHandler::wait(self)` | Blocks until the webhook receives a POST and returns `WebhookRequestPayload` |
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
      - domain: my-app.localhost:9006
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

Path variables in `{braces}` are also supported in the webhook suffix:

```moonbit
#derive.agent
#derive.mount("/api/events/{name}")
#derive.mount_webhook("/{agent-type}/callbacks/{name}")
struct EventAgent {
  name : String
}
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback

```moonbit
let webhook = @webhook.create()
let url = webhook.url()

// Share `url` with an external service (e.g., register it as a callback URL)
// The agent is durably suspended here until the external service POSTs to the URL

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
}

///|
fn IntegrationAgent::new(name : String) -> IntegrationAgent {
  { name, last_event: "" }
}

///|
/// Creates a webhook, waits for the external POST, and stores the event
#derive.endpoint(post="/register")
pub fn IntegrationAgent::register_and_wait(self : Self) -> String {
  // 1. Create a webhook
  let webhook = @webhook.create()
  let _url = webhook.url()

  // 2. In a real scenario, you would register `url` with an external service here.
  //    For this example, the URL can be retrieved and POSTed to externally.
  //    The agent is durably suspended while waiting.

  // 3. Wait for the external POST
  let payload = webhook.wait()
  let body = payload.text()

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
- The webhook URL is a one-time-use URL — once POSTed to, the promise is completed and the URL becomes invalid
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler::wait()` blocks until the callback arrives — the agent is durably suspended
- The agent survives failures, restarts, and updates while waiting
- **Never edit generated files** — `golem_reexports.mbt`, `golem_agents.mbt`, and `golem_derive.mbt` are auto-generated by `golem build`
