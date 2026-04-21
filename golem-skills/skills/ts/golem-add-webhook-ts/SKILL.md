---
name: golem-add-webhook-ts
description: "Using webhooks in a TypeScript Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a TypeScript Golem Agent

## Overview

Golem webhooks let an agent generate a temporary public URL that, when POSTed to by an external system, delivers the request body to the agent. Under the hood, a webhook is backed by a Golem promise — the agent is durably suspended while waiting for the callback, consuming no resources.

This is useful for:
- Integrating with webhook-driven APIs (payment gateways, CI/CD, GitHub, Stripe, etc.)
- Receiving asynchronous callbacks from external services
- Building event-driven workflows where an external system notifies the agent

### Prerequisites

The agent type **must** be deployed via an HTTP API mount (`mount` on `@agent()` and an `httpApi` deployment in `golem.yaml`). Without a mount, webhooks cannot be created.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-ts` | Setting up the HTTP mount and endpoint decorators required before using webhooks |
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml` |
| `golem-wait-for-external-input-ts` | Lower-level promise API if you need more control than webhooks provide |

## API

All functions/classes are in `@golemcloud/golem-ts-sdk`:

| Function / Type | Description |
|---|---|
| `createWebhook()` | Creates a webhook (promise + public URL) and returns a `WebhookHandler` |
| `WebhookHandler.getUrl()` | Returns the public URL to share with external systems |
| `WebhookHandler` (await) | Implements `PromiseLike` — use `await` to get the `WebhookRequestPayload` |
| `WebhookRequestPayload.json<T>()` | Decodes the POST body as JSON |
| `WebhookRequestPayload.bytes()` | Returns the raw POST body as `Uint8Array` |

## Imports

```typescript
import { createWebhook } from '@golemcloud/golem-ts-sdk';
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
- **`<suffix>`** — defaults to the agent type name in `kebab-case` (e.g., `OrderAgent` → `order-agent`), customizable via `webhookSuffix`
- **`<id>`** — a unique identifier for the specific webhook instance

## Webhook Suffix

You can configure a `webhookSuffix` on the `@agent()` decorator to override the default kebab-case agent name in the webhook URL:

```typescript
@agent({
  mount: '/api/orders/{id}',
  webhookSuffix: '/workflow-hooks',
})
class OrderAgent extends BaseAgent {
  // ...
}
```

Path variables in `{braces}` are also supported in `webhookSuffix`:

```typescript
@agent({
  mount: '/api/events/{name}',
  webhookSuffix: '/{agent-type}/callbacks/{name}',
})
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback

```typescript
const webhook = createWebhook();
const url = webhook.getUrl();

// Share `url` with an external service (e.g., register it as a callback URL)
// The agent is durably suspended here until the external service POSTs to the URL

const payload = await webhook;
```

### 2. Decode the Payload as JSON

```typescript
type PaymentEvent = { status: string; amount: number };

const webhook = createWebhook();
// ... share webhook.getUrl() ...
const payload = await webhook;
const event = payload.json<PaymentEvent>();
```

### 3. Use Raw Bytes

```typescript
const webhook = createWebhook();
// ... share webhook.getUrl() ...
const payload = await webhook;
const raw: Uint8Array = payload.bytes();
```

## Complete Example

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';
import { createWebhook } from '@golemcloud/golem-ts-sdk';

type WebhookEvent = { eventType: string; data: string };

@agent({ mount: '/integrations/{name}' })
class IntegrationAgent extends BaseAgent {
  private lastEvent: string = '';

  constructor(readonly name: string) {
    super();
  }

  @endpoint({ post: '/register' })
  async registerAndWait(): Promise<string> {
    // 1. Create a webhook
    const webhook = createWebhook();
    const url = webhook.getUrl();

    // 2. In a real scenario, you would register `url` with an external service here.
    //    For this example, the URL is returned so the caller can POST to it.
    //    The agent is durably suspended while awaiting.

    // 3. Wait for the external POST
    const payload = await webhook;
    const event = payload.json<WebhookEvent>();

    this.lastEvent = `${event.eventType}: ${event.data}`;
    return this.lastEvent;
  }

  @endpoint({ get: '/last-event' })
  async getLastEvent(): Promise<string> {
    return this.lastEvent;
  }
}
```

## Key Constraints

- The agent **must** have an HTTP mount (`mount` on `@agent()`) and be deployed via `httpApi` in `golem.yaml`
- The webhook URL is a one-time-use URL — once POSTed to, the promise is completed and the URL becomes invalid
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler` implements `PromiseLike`, so use `await` to wait for the callback
- The agent is durably suspended while waiting — it survives failures, restarts, and updates
