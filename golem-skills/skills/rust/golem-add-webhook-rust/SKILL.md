---
name: golem-add-webhook-rust
description: "Using webhooks in a Rust Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a Rust Golem Agent

## Overview

Golem webhooks let an agent generate a public callback URL that delivers the first POST body to the agent. A webhook is backed by a Golem promise. Creating it does not suspend execution. Awaiting an incomplete promise can durably suspend the agent after the runtime's suspension delay. Suspension releases guest execution resources; persisted state remains.

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
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml`, including `subdomain` versus `domain` |
| `golem-wait-for-external-input-rust` | Lower-level promise API if you need more control than webhooks provide |

## API

Import these APIs from `golem_rust::agentic`:

| Function / Type | Description |
|---|---|
| `create_webhook()` | Creates a webhook (promise + public URL); returns `Result<WebhookHandler, WebhookError>` |
| `WebhookHandler::url()` | Returns the public URL to share with external systems |
| `WebhookHandler` (await) | Implements `IntoFuture` — use `.await` to get the `WebhookRequestPayload` |
| `WebhookRequestPayload::json::<T>()` | Decodes the POST body as JSON (`T: DeserializeOwned`) |
| `WebhookRequestPayload::raw_data()` | Returns the raw POST body as `Vec<u8>` |

## Imports

```rust
use golem_rust::agentic::create_webhook;
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
      - subdomain: my-app  # resolves to my-app.localhost:9006 by default
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

System variables such as `{agent-type}` are supported in `webhook_suffix`; ordinary constructor
variables such as `{name}` are not:

```rust
#[agent_definition(mount = "/api/events/{name}", webhook_suffix = "/{agent-type}/callbacks")]
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback

```rust
let webhook = create_webhook().expect("webhook creation failed");
let url = webhook.url().to_string();

// Share `url` with an external service (e.g., register it as a callback URL)
// Awaiting below can suspend if the callback has not arrived.

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

let webhook = create_webhook().expect("webhook creation failed");
// ... share webhook.url() ...
let payload = webhook.await;
let event: PaymentEvent = payload.json().map_err(|e| format!("Invalid payload: {e}"))?;
```

Use this fragment inside an async function returning `Result` so malformed external JSON returns
an error rather than panicking. Completion is first-write-wins; a second POST cannot repair it.

### 3. Use Raw Bytes

```rust
let webhook = create_webhook().expect("webhook creation failed");
// ... share webhook.url() ...
let payload = webhook.await;
let raw: Vec<u8> = payload.raw_data();
```

## Complete Example

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint};
use golem_rust::agentic::create_webhook;
use serde::Deserialize;

#[derive(Deserialize)]
struct WebhookEvent {
    event_type: String,
    data: String,
}

#[agent_definition(mount = "/integrations/{name}")]
pub trait IntegrationAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/register")]
    async fn register_and_wait(&mut self) -> Result<String, String>;

    #[endpoint(get = "/last-event")]
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

    async fn register_and_wait(&mut self) -> Result<String, String> {
        // 1. Create a webhook
        let webhook = create_webhook().expect("webhook creation failed");
        let url = webhook.url().to_string();

        // The external service must accept this registration and POST the event later.
        let request = golem_rust::http::Request::post("https://events.example.com/register")
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({"callback_url": url}))
                .map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let response = golem_rust::http::send(request).await.map_err(|e| e.to_string())?;
        let status = response.status();
        let _ = golem_rust::http::response_bytes(response).await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("Registration failed: {status}"));
        }

        // 3. Wait for the external POST
        let payload = webhook.await;
        let event: WebhookEvent = payload.json().map_err(|e| format!("Invalid webhook payload: {e}"))?;

        self.last_event = format!("{}: {}", event.event_type, event.data);
        Ok(self.last_event.clone())
    }

    fn get_last_event(&self) -> String {
        self.last_event.clone()
    }
}
```

## Key Constraints

- The agent **must** have an HTTP mount (`mount = "..."` on `#[agent_definition]`) and be deployed via `httpApi` in `golem.yaml`
- The first POST completes the promise. Duplicate POSTs return HTTP 204 without replacing the first payload; the URL is not invalidated by completion.
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler` implements `IntoFuture`, so use `.await` to wait for the callback
- Use a durable agent for waits that must survive restarts; ephemeral waits are bounded and cannot durably suspend.
