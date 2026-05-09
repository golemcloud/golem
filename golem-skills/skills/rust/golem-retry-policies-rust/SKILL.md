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
    http-transient:
      priority: 10
      predicate:
        and:
          - propEq: { property: "error-type", value: "transient" }
          - propEq: { property: "uri-scheme", value: "https" }
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

Every retry decision happens in a specific **context** (an outgoing HTTP request, an HTTP
response, a worker-to-worker RPC call, a trap from inside the guest, etc.). Each context only
populates a subset of the property vocabulary below — a policy keyed on a property that is **not
present in the current context is silently skipped** for that decision (it cannot apply there by
definition).

Common to every context:
- `verb` — operation verb (HTTP method, RDBMS verb, RPC verb, or `"trap"` in the trap context)
- `noun-uri` — the resource URI (`https://...`, `worker://...`, `kv://...`, `blobstore://...`,
  `dns://...`, `wasm://<function>` for traps, `golem://api`, …)
- `uri-scheme`, `uri-host`, `uri-port`, `uri-path` — decomposed from `noun-uri`

Context-specific properties:

| Property                | Populated in                                          |
|-------------------------|-------------------------------------------------------|
| `status-code`           | outgoing HTTP **response** only                       |
| `error-type`            | outgoing HTTP **response** only                       |
| `function`              | worker-to-worker RPC call                             |
| `target-component-id`   | worker-to-worker RPC call                             |
| `target-agent-type`     | worker-to-worker RPC call (when the agent ID parses)  |
| `db-type`               | RDBMS operations (e.g. `postgres`, `mysql`)           |
| `trap-type`             | guest WASM trap (`transient-error`, `unknown`, …)     |

**Practical consequence.** A status-code-keyed policy (predicate: `status-code in [...]`) only
fires for HTTP responses. The trap path **does not** see `status-code` and silently skips that
policy — it does **not** error out. Likewise, a `trap-type`-keyed policy only fires from the
trap path. Design one policy per context (or use `or`/`and` to make a policy explicitly match
multiple contexts) rather than expecting a single policy to apply everywhere.

### `error-type` values

- `transient` — transient transport failure (e.g. WASI HTTP error code, transient RDBMS error)
- `http-status` — HTTP response with a status code that matched a `status-code`-keyed policy

### Status-code retries (opt-in)

Outgoing HTTP responses now flow through the retry-policy machinery: when the response arrives,
its `status-code` is exposed to predicates. **A policy is only considered for status-code retries
if its predicate (or the predicate inside a nested `FilteredOn`) explicitly references the
`status-code` property.** Catch-all policies — including the synthesized default and any
user-defined `Predicate::True` — are intentionally excluded so status-based retries remain
strictly opt-in.

When a matching policy decides to retry, the rejected response resource is dropped, the request
body is reconstructed from the oplog, and the request is re-sent.

Eligibility rules (mirror inline transport retry):
- live execution (not replay/snapshot/`PersistNothing`)
- request body and trailers are reconstructible
- the HTTP method is idempotent, or `assume_idempotence` was set on the outgoing request
- not inside an `atomically(...)` block — in v1 status retries are skipped inside atomic
  regions; the user-land throw still triggers atomic-region replay, which gives equivalent
  end-to-end behavior

Example status-code policy:

```yaml
http-5xx-retry:
  priority: 20
  predicate:
    and:
      - propIn: { property: "status-code", values: [500, 502, 503, 504] }
      - propEq: { property: "uri-scheme", value: "https" }
  policy:
    countBox:
      maxRetries: 3
      inner:
        exponential:
          baseDelay: "200ms"
          factor: 2.0
```

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
        .only_when(Predicate::eq(Props::ERROR_TYPE, "transient"))
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
// Match transient host-level failures
Predicate::eq(Props::ERROR_TYPE, "transient")

// Match a property value
Predicate::eq(Props::URI_SCHEME, "https")

// Combine predicates
Predicate::and(vec![
    Predicate::eq(Props::ERROR_TYPE, "transient"),
    Predicate::eq(Props::URI_SCHEME, "https"),
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
  --predicate '{ "and": [{ "propEq": { "property": "error-type", "value": "transient" } }, { "propEq": { "property": "uri-scheme", "value": "https" } }] }' \
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
