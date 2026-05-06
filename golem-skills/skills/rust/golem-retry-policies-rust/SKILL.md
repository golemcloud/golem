---
name: golem-retry-policies-rust
description: "Configuring semantic retry policies for a Rust Golem agent. Use when the user asks about retry policies, retry strategies, exponential backoff, error handling retries, transient error recovery, retry predicates, withRetryPolicy, with_named_policy, NamedPolicy, Policy composition, jitter, countBox, timeBox, andThen, or customizing how failures are retried."
---

# Configuring Semantic Retry Policies (Rust)

Golem provides a composable, per-environment retry policy system. Policies are evaluated against error context properties and can be defined in the application manifest, managed via CLI, or created/overridden at runtime from agent code using the SDK.

## 1. Define Retry Policies in the Application Manifest

Add retry policy definitions under `retryPolicyDefaults` in `golem.yaml`, scoped per environment:

```yaml
retryPolicyDefaults:
  prod:
    no-retry-4xx:
      priority: 20
      predicate:
        and:
          - propGte: { property: "status-code", value: 400 }
          - propLt: { property: "status-code", value: 500 }
      policy:
        never: {}

    http-transient:
      priority: 10
      predicate:
        propIn: { property: "status-code", values: [502, 503, 504] }
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

### Policy Evaluation Order

When an error occurs, policies are evaluated in **descending priority order**. The first matching predicate's policy is applied. If no user-defined policy matches, the built-in default policy (3 retries, exponential backoff, clamped to [100ms, 1s], 15% jitter) is used.

### Base Policies

| Policy | Description |
|--------|-------------|
| `periodic` | Fixed delay between each attempt |
| `exponential` | `baseDelay × factor^attempt` — exponentially growing delays |
| `fibonacci` | Delays follow the Fibonacci sequence starting from `first` and `second` |
| `immediate` | Retry immediately (zero delay) |
| `never` | Never retry — give up on first failure |

### Combinators

| Combinator | Description |
|------------|-------------|
| `countBox` | Limits the total number of retry attempts |
| `timeBox` | Limits retries to a wall-clock duration |
| `clamp` | Clamps computed delay to a `[minDelay, maxDelay]` range |
| `addDelay` | Adds a constant offset on top of the computed delay |
| `jitter` | Adds random noise (±factor × delay) to avoid thundering herds |
| `filteredOn` | Apply the inner policy only when a predicate matches; otherwise give up |
| `andThen` | Run the first policy until it gives up, then switch to the second |
| `union` | Retry if *either* sub-policy wants to; pick the shorter delay |
| `intersect` | Retry only while *both* sub-policies want to; pick the longer delay |

### Predicates

Predicates are boolean expressions evaluated against error context properties. Compose with `and`, `or`, `not`:

- `true` / `false` — always/never match
- `propEq` — property equals a value
- `propIn` — property is one of a set of values
- `propGte` / `propLt` — numeric comparisons
- `and` / `or` / `not` — logical composition

### Available Properties

- `status-code` — HTTP response status code
- `uri-scheme` — URI scheme (http, https)
- `error-type` — classification of the error

## 2. SDK: Build and Apply Retry Policies at Runtime

Use `golem_rust::retry` to construct and apply retry policies from agent code:

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
```

### Scoped Usage with `with_named_policy`

Apply a policy for a block of code — the previous policy is restored when the block exits:

```rust
with_named_policy(&policy, || {
    // HTTP calls in this block use the custom retry policy
    make_http_request();
})?;
```

### Policy Builder Methods

Build policies fluently from base policies:

```rust
// Exponential backoff clamped with jitter and max retries
Policy::exponential(Duration::from_millis(200), 2.0)
    .clamp(Duration::from_millis(100), Duration::from_secs(5))
    .with_jitter(0.15)
    .max_retries(5)

// Periodic with time limit
Policy::periodic(Duration::from_secs(1))
    .time_box(Duration::from_secs(60))

// Immediate retries then fall back to exponential
Policy::immediate()
    .max_retries(3)
    .and_then(
        Policy::exponential(Duration::from_secs(1), 2.0)
            .max_retries(5)
    )

// Never retry (fail immediately)
Policy::never()
```

### Predicate Builder Methods

```rust
// Match specific status codes
Predicate::one_of(Props::STATUS_CODE, [502_u16, 503, 504])

// Match a property value
Predicate::eq(Props::URI_SCHEME, "https")

// Combine predicates
Predicate::and(vec![
    Predicate::gte(Props::STATUS_CODE, 500_u16),
    Predicate::lt(Props::STATUS_CODE, 600_u16),
])
```

## 3. Querying Retry Policies at Runtime

Use the query API to inspect active policies from agent code:

```rust
use golem_rust::retry::{get_retry_policies, get_retry_policy_by_name};

// List all active policies
let policies = get_retry_policies();
for p in &policies {
    log::info!("Policy '{}' priority={}", p.name, p.priority);
}

// Get a specific policy by name
if let Some(policy) = get_retry_policy_by_name("http-transient") {
    log::info!("Found policy with priority {}", policy.priority);
}
```

The returned `NamedRetryPolicy` has fields: `name` (String), `priority` (u32), `predicate` (RetryPredicate), `policy` (RetryPolicy).

## 4. Live-Editing Policies via CLI

Retry policies can be managed at runtime without redeployment:

```shell
# Create a new policy
golem retry-policy create http-transient \
  --priority 10 \
  --predicate '{ "propIn": { "property": "status-code", "values": [502, 503, 504] } }' \
  --policy '{ "countBox": { "maxRetries": 5, "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } } } }'

# List all policies in the current environment
golem retry-policy list

# Get a specific policy by name
golem retry-policy get http-transient

# Update an existing policy
golem retry-policy update http-transient --priority 15

# Delete a policy
golem retry-policy delete http-transient
```

## 5. Default Retry Policy

When no user-defined retry policies are set, Golem activates a default catch-all:

- **Name**: `default`
- **Priority**: `0`
- **Predicate**: `true` (matches everything)
- **Policy**: Up to 3 retries, exponential backoff (factor 3.0), delays clamped to [100ms, 1s], 15% jitter

## Key Constraints

- Policies are defined **per-environment** — different environments can have different retry behaviors
- Policy names must be unique within an environment
- Higher priority policies are evaluated first; the first matching predicate wins
- `with_named_policy` is scoped — the policy is restored when the closure exits
- Inline retries (automatic transparent retries for transient network errors) happen before the policy system kicks in
- Changes made via CLI or REST API take effect immediately for running agents
