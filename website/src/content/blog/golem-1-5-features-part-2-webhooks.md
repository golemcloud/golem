---
title: "Golem 1.5 features — Part 2: Webhooks"
date: "2026-04-10T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-2-webhooks"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part2-webhooks/"
---

## Introduction

A series of concise posts showcasing Golem 1.5's new capabilities, releasing end of April 2026. The episodes of this series are short and assume the reader knows what Golem is. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Webhooks

Building on HTTP API mapping from the prior installment, Golem 1.5 introduces webhook creation and awaiting functionality. Webhooks are built on top of Golem Promises, which were available in previous Golem releases as well.

### Creating a webhook

```typescript
const webhook = createWebhook();
const url = webhook.getUrl();

// At this point we can somehow advertise this `url` - return as a result, post to a 3rd party API, etc

const payload = await webhook; // block until someone calls the webhook with a payload
const result: T = payload.json();
```

```rust
let webhook = create_webhook();
let url = webhook.url();

// At this point we can somehow advertise this `url` - return as a result, post to a 3rd party API, etc

let request = webhook.await;
let data: T = request.json().unwrap();
```

```scala
val webhook = HostApi.createWebhook()
val url = webhook.url

// At this point we can somehow advertise this `url` - return as a result, post to a 3rd party API, etc

webhook.await().map { payload =>
  val event = payload.json[T]()
  // ...
}
```

```moonbit
let webhook = @webhook.create()
let url = webhook.url()

// At this point we can somehow advertise this `url` - return as a result, post to a 3rd party API, etc

let payload = webhook.wait()
let text = payload.text()
```

### Calling the webhook

Webhooks accept POST requests with arbitrary body content, accessible via payload helper methods.

### Customizing the webhook URL

Webhook URLs follow this structure:

```
https://<domain>/<prefix>/<suffix>/<id>
```

Configure the prefix in the deployment manifest:

```yaml
httpApi:
  deployments:
    default:
      - domain: example.com
        webhookUrl: "/my-custom-webhooks/"
        agents:
          # ...
```

Set a custom suffix via mount point configuration across the supported languages:

```typescript
@agent({
  mount: "/workflow/{id}",
  webhookSuffix: "/workflow-hooks",
})
class Workflow extends BaseAgent {
  // ...
}
```

```rust
#[agent_definition(
    mount = "/workflow/{id}",
    webhook_suffix = "/workflow-hooks"
)]
pub trait Workflow {
    // ...
}
```

```scala
@agentDefinition(
  mount = "/workflow/{id}",
  webhookSuffix = "/workflow-hooks",
)
trait Workflow extends BaseAgent {
  // ...
}
```

```moonbit
#derive.agent
#derive.mount("/workflow/{id}")
#derive.mount_webhook_suffix("/workflow-hooks")
pub(all) struct Workflow {
  // ...
}
```
