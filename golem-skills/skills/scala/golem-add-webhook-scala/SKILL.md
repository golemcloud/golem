---
name: golem-add-webhook-scala
description: "Using webhooks in a Scala Golem agent. Use when the user asks to create webhooks, receive webhook callbacks, integrate with webhook-driven external APIs, or generate temporary callback URLs for external services."
---

# Using Webhooks in a Scala Golem Agent

## Overview

Golem webhooks let an agent generate a temporary public URL that, when POSTed to by an external system, delivers the request body to the agent. Under the hood, a webhook is backed by a Golem promise — the agent is durably suspended while waiting for the callback, consuming no resources.

This is useful for:
- Integrating with webhook-driven APIs (payment gateways, CI/CD, GitHub, Stripe, etc.)
- Receiving asynchronous callbacks from external services
- Building event-driven workflows where an external system notifies the agent

### Prerequisites

The agent type **must** be deployed via an HTTP API mount (`mount = "/..."` on `@agentDefinition` and an `httpApi` deployment in `golem.yaml`). Without a mount, webhooks cannot be created.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-scala` | Setting up the HTTP mount and endpoint annotations required before using webhooks |
| `golem-configure-api-domain` | Configuring `httpApi` in `golem.yaml` |
| `golem-wait-for-external-input-scala` | Lower-level promise API if you need more control than webhooks provide |

## API

All functions are on the `golem.HostApi` object:

| Function / Type | Description |
|---|---|
| `HostApi.createWebhook()` | Creates a webhook (promise + public URL) and returns a `WebhookHandler` |
| `WebhookHandler.url` | The public URL to share with external systems |
| `WebhookHandler.await()` | Awaits the webhook POST payload asynchronously (`Future[WebhookRequestPayload]`) |
| `WebhookHandler.awaitBlocking()` | Blocks until the webhook POST payload arrives (`WebhookRequestPayload`) |
| `WebhookRequestPayload.json[A]()` | Decodes the POST body as JSON (requires implicit `Schema[A]`) |
| `WebhookRequestPayload.bytes` | Returns the raw POST body as `Array[Byte]` |

## Imports

```scala
import golem.HostApi
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

You can configure a `webhookSuffix` on the `@agentDefinition` annotation to override the default kebab-case agent name in the webhook URL:

```scala
@agentDefinition(mount = "/api/orders/{id}", webhookSuffix = "/workflow-hooks")
trait OrderAgent extends BaseAgent {
  class Id(val id: String)
  // ...
}
```

Path variables in `{braces}` are also supported in `webhookSuffix`:

```scala
@agentDefinition(mount = "/api/events/{name}", webhookSuffix = "/{agent-type}/callbacks/{name}")
```

## Usage Pattern

### 1. Create a Webhook, Share the URL, and Await the Callback (Blocking)

```scala
val webhook = HostApi.createWebhook()
val url = webhook.url

// Share `url` with an external service (e.g., register it as a callback URL)
// The agent is durably suspended here until the external service POSTs to the URL

val payload = webhook.awaitBlocking()
```

### 2. Create a Webhook and Await Asynchronously

```scala
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

val webhook = HostApi.createWebhook()
val url = webhook.url
// ... share url ...

val result: Future[WebhookRequestPayload] = webhook.await()
result.map { payload =>
  val event = payload.json[MyEvent]()
  // process event
}
```

### 3. Decode the Payload as JSON

```scala
import zio.blocks.schema.Schema

case class PaymentEvent(status: String, amount: Long) derives Schema

val webhook = HostApi.createWebhook()
// ... share webhook.url ...
val payload = webhook.awaitBlocking()
val event = payload.json[PaymentEvent]()
```

### 4. Use Raw Bytes

```scala
val webhook = HostApi.createWebhook()
// ... share webhook.url ...
val payload = webhook.awaitBlocking()
val raw: Array[Byte] = payload.bytes
```

## Complete Example

```scala
import golem.*
import golem.runtime.annotations.{agentDefinition, agentImplementation, endpoint}
import zio.blocks.schema.Schema

import scala.concurrent.Future

case class WebhookEvent(eventType: String, data: String) derives Schema

@agentDefinition(mount = "/integrations/{name}")
trait IntegrationAgent extends BaseAgent {
  class Id(val name: String)

  @endpoint(method = "POST", path = "/register")
  def registerAndWait(): String

  @endpoint(method = "GET", path = "/last-event")
  def getLastEvent(): String
}

@agentImplementation()
class IntegrationAgentImpl extends IntegrationAgent {
  private var name: String = ""
  private var lastEvent: String = ""

  override def init(id: Id): Unit = {
    name = id.name
  }

  override def registerAndWait(): String = {
    // 1. Create a webhook
    val webhook = HostApi.createWebhook()
    val url = webhook.url

    // 2. In a real scenario, you would register `url` with an external service here.
    //    For this example, the URL is returned so the caller can POST to it.
    //    The agent is durably suspended while awaiting.

    // 3. Wait for the external POST
    val payload = webhook.awaitBlocking()
    val event = payload.json[WebhookEvent]()

    lastEvent = s"${event.eventType}: ${event.data}"
    lastEvent
  }

  override def getLastEvent(): String = lastEvent
}
```

## Key Constraints

- The agent **must** have an HTTP mount (`mount = "..."` on `@agentDefinition`) and be deployed via `httpApi` in `golem.yaml`
- The webhook URL is a one-time-use URL — once POSTed to, the promise is completed and the URL becomes invalid
- Only `POST` requests to the webhook URL will complete the promise
- Use `awaitBlocking()` from synchronous code paths; use `await()` for `Future`-based async patterns
- The agent is durably suspended while waiting — it survives failures, restarts, and updates
- JSON decoding requires an implicit `zio.blocks.schema.Schema[A]` instance (use `derives Schema` in Scala 3)
