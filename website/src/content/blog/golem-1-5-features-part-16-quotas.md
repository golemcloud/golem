---
title: "Golem 1.5 features — Part 16: Quotas"
date: "2026-04-24T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-16-quotas"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part16-quotas/"
---

## Introduction

This is part of a series of brief posts about Golem 1.5, releasing at the end of April 2026. The piece assumes reader familiarity with Golem and references other related posts for additional context. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Quotas

Modern applications rely on third-party services, particularly AI agents that interact with external systems and LLM providers. These services typically impose costs and usage limits. Golem 1.5 introduces quota management to help developers control the parallel running agents and make sure we don't over-use the limited resources.

The feature allows developers to define resources with limited availability and enforce reservations through quota tokens. Tokens can be split and passed between agents via RPC calls.

### Setting Up Resources

Resources are defined per environment in the application manifest. Three limit types exist:

- **Rate**: Refillable pools that replenish by a specified value within a period
- **Capacity**: Fixed tokens that never refill once consumed
- **Concurrency**: Fixed pools where agents can temporarily reserve tokens

Enforcement actions include `reject` (return error), `throttle` (suspend agent), or `terminate` (kill agent).

```yaml
resourceDefaults:
  prod:
    api-calls:
      limit:
        type: Rate
        value: 100
        period: minute
        max: 1000
      enforcementAction: reject
      unit: request
      units: requests
    storage:
      limit:
        type: Capacity
        value: 1073741824
      enforcementAction: reject
      unit: byte
      units: bytes
    connections:
      limit:
        type: Concurrency
        value: 50
      enforcementAction: throttle
      unit: connection
      units: connections
```

### Dynamic Management

Resource limits can be modified via CLI commands or REST API calls, affecting running agents immediately without redeployment.

### Token acquisition

```typescript
import { acquireQuotaToken } from "golem-ts-sdk";

const token = acquireQuotaToken("api-calls", 1n);
```

```rust
use golem_rust::quota::QuotaToken;

let token = QuotaToken::new("api-calls", 1);
```

```scala
import golem.host.QuotaApi._

val token = QuotaToken("api-calls", BigInt(1))
```

```moonbit
let token = @quota.QuotaToken::new("api-calls", 1UL)
```

### Simple rate limiting with `withReservation`

```typescript
import { withReservation } from "golem-ts-sdk";

const result = await withReservation(token, 1n, async (reservation) => {
  const response = await callSimpleApi();
  return { used: BigInt(1), value: response };
});
```

```rust
use golem_rust::quota::with_reservation;

let result = with_reservation(&token, 1, |_reservation| {
    let response = call_simple_api();
    (1, response)
});
```

```scala
val result = withReservation(token, BigInt(1)) { reservation =>
  callSimpleApi().map { response =>
    (BigInt(1), response)
  }
}
```

```moonbit
let result = @quota.with_reservation(token, 1UL, fn(reservation) {
  let response = callSimpleApi()
  (1, response)
})
```

### LLM rate limiting based on token consumption

<!-- WebFetch did not return a dedicated code example for the LLM rate limiting section; consult the original post if specific code is required. -->

### Token splitting and merging

```typescript
const childToken: QuotaToken = token.split(200n);

const childAgent = await SummarizerAgent.newPhantom();
const summary = childAgent.summarize(text, childToken);
```

```rust
let child_token: QuotaToken = token.split(200);

let child_agent = SummarizerAgent::new_phantom().await;
let summary = child_agent.summarize(text, child_token).await;
```

```scala
val childToken: QuotaToken = token.split(BigInt(200))

for {
  childAgent <- SummarizerAgent.newPhantom()
  summary <- childAgent.summarize(text, childToken)
} yield summary
```

```moonbit
let child_token: QuotaToken = self.token.split(200UL)

let child_agent = SummarizerAgent::new_phantom()
child_agent.summarize(text, child_token)
```
