---
name: golem-add-webhook-rust
description: "Using webhooks in a Rust Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a Rust Golem Agent

## Overview

Golem webhooks let an agent generate a temporary public URL that, when POSTed to by an external system, delivers the request body to the agent. Under the hood, a webhook is backed by a Golem promise — the agent is durably suspended while waiting for the callback, consuming no resources.

This is useful for:
- Integrating with webhook-driven APIs (payment gateways, CI/CD, GitHub, Stripe, etc.)
- Receiving asynchronous callbacks from external services
- Building event-driven workflows where an external system notifies the agent

### Prerequisites

The agent type **must** be deployed via an HTTP API mount (`mount = "/..."` on `#[agent_definition]` and an `httpApi` deployment in `golem.yaml`). Without a mount, webhooks cannot be created.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-rust` | Setting up the HTTP mount and endpoint annotations required before using webhooks |
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml` |
| `golem-wait-for-external-input-rust` | Lower-level promise API if you need more control than webhooks provide |

## API

All functions are in the `golem_rust` crate:

| Function / Type | Description |
|---|---|
| `create_webhook()` | Creates a webhook (promise + public URL) and returns a `WebhookHandler` |
| `WebhookHandler::url()` | Returns the public URL to share with external systems |
| `WebhookHandler` (await) | Implements `IntoFuture` — use `.await` to get the `WebhookRequestPayload` |
| `WebhookRequestPayload::json::<T>()` | Decodes the POST body as JSON (`T: DeserializeOwned`) |
| `WebhookRequestPayload::raw_data()` | Returns the raw POST body as `Vec<u8>` |

## Imports

```rust
use golem_rust::create_webhook;
```

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
- **`<suffix>`** — defaults to the agent type name in `kebab-case` (e.g., `OrderAgent` → `order-agent`), customizable via `webhook_suffix`
- **`<id>`** — a unique identifier for the specific webhook instance

## Webhook Suffix

You can configure a `webhook_suffix` on the `#[agent_definition]` to override the default kebab-case agent name in the webhook URL:

```rust
#[agent_definition(mount = "/api/orders/{id}", webhook_suffix = "/workflow-hooks")]
pub trait OrderAgent {
    fn new(id: String) -> Self;
    // ...
}
```

Path variables in `{braces}` are also supported in `webhook_suffix`:

```rust
#[agent_definition(mount = "/api/events/{name}", webhook_suffix = "/{agent-type}/callbacks/{name}")]
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback

```rust
let webhook = create_webhook();
let url = webhook.url().to_string();

// Share `url` with an external service (e.g., register it as a callback URL)
// The agent is durably suspended here until the external service POSTs to the URL

let payload = webhook.await;
```

### 2. Decode the Payload as JSON

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct PaymentEvent {
    status: String,
    amount: u64,
}

let webhook = create_webhook();
// ... share webhook.url() ...
let payload = webhook.await;
let event: PaymentEvent = payload.json().expect("Invalid payload");
```

### 3. Use Raw Bytes

```rust
let webhook = create_webhook();
// ... share webhook.url() ...
let payload = webhook.await;
let raw: Vec<u8> = payload.raw_data();
```

## Complete Example

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint, Schema, create_webhook};
use serde::Deserialize;

#[derive(Deserialize)]
struct WebhookEvent {
    event_type: String,
    data: String,
}

#[agent_definition(mount = "/integrations/{name}")]
pub trait IntegrationAgent {
    fn new(name: String) -> Self;

    async fn register_and_wait(&mut self) -> String;

    fn get_last_event(&self) -> String;
}

struct IntegrationAgentImpl {
    name: String,
    last_event: String,
}

#[agent_implementation]
impl IntegrationAgent for IntegrationAgentImpl {
    fn new(name: String) -> Self {
        Self {
            name,
            last_event: String::new(),
        }
    }

    #[endpoint(post = "/register")]
    async fn register_and_wait(&mut self) -> String {
        // 1. Create a webhook
        let webhook = create_webhook();
        let url = webhook.url().to_string();

        // 2. In a real scenario, you would register `url` with an external service here.
        //    For this example, the URL is returned so the caller can POST to it.
        //    The agent is durably suspended while awaiting.

        // 3. Wait for the external POST
        let payload = webhook.await;
        let event: WebhookEvent = payload.json().expect("Invalid webhook payload");

        self.last_event = format!("{}: {}", event.event_type, event.data);
        self.last_event.clone()
    }

    #[endpoint(get = "/last-event")]
    fn get_last_event(&self) -> String {
        self.last_event.clone()
    }
}
```

## Key Constraints

- The agent **must** have an HTTP mount (`mount = "..."` on `#[agent_definition]`) and be deployed via `httpApi` in `golem.yaml`
- The webhook URL is a one-time-use URL — once POSTed to, the promise is completed and the URL becomes invalid
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler` implements `IntoFuture`, so use `.await` to wait for the callback
- The agent is durably suspended while waiting — it survives failures, restarts, and updates
