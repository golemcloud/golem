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

The agent type **must** be deployed via an HTTP API mount (`http.mount(...)` on `defineAgent(...)` and an `httpApi` deployment in `golem.yaml`). Without a mount, webhooks cannot be created.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-ts` | Setting up the `http.mount(...)` and endpoint declarations required before using webhooks |
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml`, including `subdomain` versus `domain` |
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
      - subdomain: my-app  # resolves to my-app.localhost:9006 by default
        webhookUrl: "/my-custom-webhooks/"
        agents:
          OrderAgent: {}
  ```
- **`<suffix>`** — defaults to the agent type name in `kebab-case` (e.g., `OrderAgent` → `order-agent`), customizable via `webhookSuffix`
- **`<id>`** — a unique identifier for the specific webhook instance

## Webhook Suffix

You can configure a `webhookSuffix` in the mount options of `http.mount(...)` to override the default kebab-case agent name in the webhook URL:

```typescript
export const OrderAgent = defineAgent({
  name: 'OrderAgent',
  id: { id: z.string() },
  http: http.mount('/api/orders/{id}', { webhookSuffix: '/workflow-hooks' }),
  methods: { /* ... */ },
});
```

Path variables in `{braces}` (and system variables like `{agent-type}`) are also supported in `webhookSuffix`:

```typescript
http: http.mount('/api/events/{name}', { webhookSuffix: '/{agent-type}/callbacks/{name}' }),
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
import { z } from 'zod';
import { defineAgent, method, http, createWebhook } from '@golemcloud/golem-ts-sdk';

type WebhookEvent = { eventType: string; data: string };

export const IntegrationAgent = defineAgent({
  name: 'IntegrationAgent',
  id: { name: z.string() },
  http: http.mount('/integrations/{name}'),
  methods: {
    registerAndWait: method({ input: {}, returns: z.string(), http: http.post('/register') }),
    getLastEvent: method({ input: {}, returns: z.string(), http: http.get('/last-event') }),
  },
});

export const IntegrationAgentImpl = IntegrationAgent.implement({
  init: () => ({ lastEvent: '' }),
  methods: {
    async registerAndWait() {
      // 1. Create a webhook
      const webhook = createWebhook();
      const url = webhook.getUrl();

      // 2. In a real scenario, you would register `url` with an external service here.
      //    For this example, the URL is created so the caller can POST to it.
      //    The agent is durably suspended while awaiting.

      // 3. Wait for the external POST
      const payload = await webhook;
      const event = payload.json<WebhookEvent>();

      this.lastEvent = `${event.eventType}: ${event.data}`;
      return this.lastEvent;
    },
    getLastEvent() {
      return this.lastEvent;
    },
  },
});
```

## Key Constraints

- The agent **must** have an HTTP mount (`http.mount(...)` on `defineAgent(...)`) and be deployed via `httpApi` in `golem.yaml`
- The webhook URL is a one-time-use URL — once POSTed to, the promise is completed and the URL becomes invalid
- Only `POST` requests to the webhook URL will complete the promise
- `WebhookHandler` implements `PromiseLike`, so use `await` to wait for the callback
- The agent is durably suspended while waiting — it survives failures, restarts, and updates
