---
name: golem-retry-policies-ts
description: "Configuring semantic retry policies for a TypeScript Golem agent. Use when the user asks about retry policies, retry strategies, exponential backoff, error handling retries, transient error recovery, retry predicates, withRetryPolicy, NamedPolicy, Policy composition, jitter, countBox, timeBox, andThen, or customizing how failures are retried."
---

# Configuring Semantic Retry Policies (TypeScript)

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

Use `@golemcloud/golem-ts-sdk` to construct and apply retry policies from agent code:

```typescript
import {
  Policy, Predicate, NamedPolicy, Props, Duration,
  withRetryPolicy,
} from '@golemcloud/golem-ts-sdk';

const policy = NamedPolicy.named(
  'http-transient',
  Policy.exponential(Duration.milliseconds(200), 2.0)
    .clamp(Duration.milliseconds(100), Duration.seconds(5))
    .withJitter(0.15)
    .onlyWhen(Predicate.oneOf(Props.statusCode, [502, 503, 504]))
    .maxRetries(5),
)
  .priority(10)
  .appliesWhen(Predicate.eq(Props.uriScheme, 'https'));
```

### Scoped Usage with `withRetryPolicy`

Apply a policy for a block of code — the previous policy is restored when the block exits:

```typescript
withRetryPolicy(policy, () => {
  // HTTP calls in this block use the custom retry policy
  makeHttpRequest();
});
```

### Policy Builder Methods

Build policies fluently from base policies:

```typescript
// Exponential backoff clamped with jitter and max retries
Policy.exponential(Duration.milliseconds(200), 2.0)
  .clamp(Duration.milliseconds(100), Duration.seconds(5))
  .withJitter(0.15)
  .maxRetries(5)

// Periodic with time limit
Policy.periodic(Duration.seconds(1))
  .timeBox(Duration.seconds(60))

// Immediate retries then fall back to exponential
Policy.immediate()
  .maxRetries(3)
  .andThen(
    Policy.exponential(Duration.seconds(1), 2.0)
      .maxRetries(5)
  )

// Never retry (fail immediately)
Policy.never()
```

### Predicate Builder Methods

```typescript
// Match specific status codes
Predicate.oneOf(Props.statusCode, [502, 503, 504])

// Match a property value
Predicate.eq(Props.uriScheme, 'https')

// Combine predicates
Predicate.and([
  Predicate.gte(Props.statusCode, 500),
  Predicate.lt(Props.statusCode, 600),
])
```

## 3. Querying Retry Policies at Runtime

Use the query API to inspect active policies from agent code:

```typescript
import { getRetryPolicies, getRetryPolicyByName } from '@golemcloud/golem-ts-sdk';

// List all active policies
const policies = getRetryPolicies();
for (const p of policies) {
  console.log(`Policy '${p.name}' priority=${p.priority}`);
}

// Get a specific policy by name
const policy = getRetryPolicyByName('http-transient');
if (policy) {
  console.log(`Found policy with priority ${policy.priority}`);
}
```

The returned `NamedRetryPolicy` has fields: `name` (string), `priority` (number), `predicate` (RetryPredicate), `policy` (RetryPolicy).

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
- `withRetryPolicy` is scoped — the policy is restored when the callback exits
- Inline retries (automatic transparent retries for transient network errors) happen before the policy system kicks in
- Changes made via CLI or REST API take effect immediately for running agents
