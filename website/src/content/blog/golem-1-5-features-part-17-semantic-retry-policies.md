---
title: "Golem 1.5 features — Part 17: Semantic retry policies"
date: "2026-04-24T16:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-17-semantic-retry-policies"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part17-semantic-retry-policies/"
---

## Introduction

This post is part of a series showcasing Golem 1.5 features, releasing at the end of April 2026. The series assumes readers understand what Golem is. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Retry policies

Previous Golem versions featured a simple global retry policy with exponential backoff that applied uniformly to all failures. Golem 1.5 improves this through two mechanisms: automatic inline retries in many scenarios and a fully redesigned, customizable and composable retry policy model.

### Inline retries

The system now transparently retries transient issues like HTTP requests immediately without recreating agent instances. Additionally, Golem better classifies which failures can be retried versus those that will deterministically fail again.

### Retry policies

Unlike the previous single global configuration, new retry policies are defined per-environment with arbitrary named policies. Each policy contains a predicate determining applicability and a policy describing the retry strategy.

#### Policies

Base policy nodes controlling delays include:

- `periodic`: Fixed delay between attempts
- `exponential`: `baseDelay × factor^attempt`
- `fibonacci`: Fibonacci sequence delays
- `immediate`: Zero delay retries
- `never`: Never retry

Combinators allow composition:

- `countBox`: Limit total retry attempts
- `timeBox`: Limit retries to wall-clock duration
- `clamp`: Constrain delay to `[minDelay, maxDelay]`
- `addDelay`: Add constant offset
- `jitter`: Add random noise to prevent thundering herds
- `filteredOn`: Apply policy conditionally
- `andThen`: Switch policies sequentially
- `union`: Retry if either sub-policy wants to
- `intersect`: Retry only while both sub-policies want to

#### Predicates

Predicates evaluate against error context properties using boolean logic:

- `propEq` / `propNeq`: Equality comparisons
- `propGt` / `propGte` / `propLt` / `propLte`: Numeric comparisons
- `propExists`: Property existence check
- `propIn`: Value in given set
- `propMatches`: Glob pattern matching
- `propStartsWith` / `propContains`: String matching
- `true` / `false`: Constant predicates

#### Available properties

Context properties include:

- `verb`: HTTP method or action type
- `noun-uri`: Target URI
- `uri-scheme`, `uri-host`, `uri-port`, `uri-path`: URI components
- `status-code`: HTTP response code
- `error-type`: Error classification
- `function`: RPC target function
- `target-component-id`: Worker component ID
- `target-agent-type`: Agent type name
- `db-type`: Database type
- `trap-type`: WASM trap classification

### Defining policies

Policies are defined in the application manifest under `retryPolicyDefaults`, keyed by environment:

```yaml
retryPolicyDefaults:
  my-environment:
    no-retry-4xx:
      priority: 20
      predicate:
        and:
          - propGte: { property: status-code, value: 400 }
          - propLt: { property: status-code, value: 500 }
      policy: "never"

    http-transient:
      priority: 10
      predicate:
        propIn:
          property: status-code
          values: [502, 503, 504]
      policy:
        countBox:
          maxRetries: 5
          inner:
            jitter:
              factor: 0.15
              inner:
                clamp:
                  minDelay: "100ms"
                  maxDelay: "5s"
                  inner:
                    exponential:
                      baseDelay: "200ms"
                      factor: 2.0

    catch-all:
      priority: 0
      predicate: true
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential:
              baseDelay: "100ms"
              factor: 3.0
```

Policies evaluate in descending priority order, allowing granular control over retry behavior for different failure types.

### Default retry policy

When no user-defined policies exist, a default catch-all activates matching previous behavior:

- **Name**: `"default"`
- **Priority**: `0`
- **Predicate**: `true`
- **Policy**: Up to 3 retries, exponential backoff with factor 3.0, delays clamped to [100ms, 1s], with 15% jitter

```yaml
name: default
priority: 0
predicate: true
policy:
  countBox:
    maxRetries: 3
    inner:
      jitter:
        factor: 0.15
        inner:
          clamp:
            minDelay: "100ms"
            maxDelay: "1s"
            inner:
              exponential:
                baseDelay: "100ms"
                factor: 3.0
```

### Live-editing policies

Default policies can be modified via CLI or REST API without redeployment:

```bash
# Create a new policy
golem retry-policy create http-transient \
  --priority 10 \
  --predicate '{ propIn: { property: "status-code", values: [502, 503, 504] } }' \
  --policy '{ countBox: { maxRetries: 5, inner: { exponential: { baseDelay: "200ms", factor: 2.0 } } } }'

# List all policies
golem retry-policy list

# Get specific policy
golem retry-policy get http-transient

# Update policy
golem retry-policy update http-transient --priority 15

# Delete policy
golem retry-policy delete http-transient
```

#### SDK support

The Golem SDK provides runtime query and modification capabilities:

```typescript
import {
  Policy,
  Predicate,
  NamedPolicy,
  Props,
  Duration,
  withRetryPolicy,
} from "@golemcloud/golem-ts-sdk";

const policy = NamedPolicy.named(
  "http-transient",
  Policy.exponential(Duration.milliseconds(200), 2.0)
    .clamp(Duration.milliseconds(100), Duration.seconds(5))
    .withJitter(0.15)
    .onlyWhen(Predicate.oneOf(Props.statusCode, [502, 503, 504]))
    .maxRetries(5)
)
  .priority(10)
  .appliesWhen(Predicate.eq(Props.uriScheme, "https"));

withRetryPolicy(policy, () => {
  makeHttpRequest();
});
```

```rust
use golem_rust::retry::*;
use std::time::Duration;

let policy = NamedPolicy::named(
    "http-transient",
    Policy::exponential(Duration::from_millis(200), 2.0)
        .clamp(Duration::from_millis(100), Duration::from_secs(5))
        .with_jitter(0.15)
        .only_when(Predicate::one_of(Props::STATUS_CODE, [502_u16, 503, 504]))
        .max_retries(5),
)
.priority(10)
.applies_when(Predicate::eq(Props::URI_SCHEME, "https"));

with_named_policy(&policy, || {
    make_http_request();
})?;
```

```scala
import golem.Guards._
import golem.host.Retry._

import scala.concurrent.duration._

val policy = named(
  "http-transient",
  Policy.exponential(200.millis, 2.0)
    .clamp(100.millis, 5.seconds)
    .withJitter(0.15)
    .onlyWhen(Props.statusCode.oneOf(502, 503, 504))
    .maxRetries(5)
).priority(10)
 .appliesWhen(Props.uriScheme.eq("https"))

withRetryPolicy(policy) {
  Future {
    makeHttpRequest()
  }
}
```

```moonbit
let policy =
  NamedPolicy::named(
    "http-transient",
    Policy::exponential(Duration::millis(200), 2.0)
      .clamp(Duration::millis(100), Duration::seconds(5))
      .with_jitter(0.15)
      .only_when(
        Predicate::one_of(
          Props::status_code(),
          [Value::int(502), Value::int(503), Value::int(504)],
        ),
      )
      .max_retries(5),
  )
    .priority(10)
    .applies_when(Predicate::eq(Props::uri_scheme(), Value::text("https")))

with_named_policy!(policy, fn() {
  make_http_request()
})
```

### Extensibility

Future retry-capable host functionality integrates seamlessly into this system. Third-party and user-level retry functionalities can build upon the ability to query policies at runtime.
