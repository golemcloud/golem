---
name: golem-quota-rust
description: "Adding resource quotas to a Rust Golem agent. Use when the user asks about rate limiting, resource quotas, quota tokens, QuotaToken, reservations, throttling API calls, limiting concurrency, capacity limits, or splitting tokens between agents."
---

# Adding Resource Quotas to an Agent (Rust)

Golem provides a distributed resource quota system via `golem_rust::quota`. Quotas let you define limited resources (API call rates, storage capacity, connection concurrency) and enforce consumption limits across all agents in a deployment.

## 1. Define Resources in the Application Manifest

Add resource definitions under `resourceDefaults` in `golem.yaml`, scoped per environment:

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
        value: 1073741824  # 1 GB
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

### Limit Types

- **`Rate`** — refills `value` tokens every `period` (second/minute/hour/day), capped at `max`. Use for rate-limiting API calls.
- **`Capacity`** — fixed pool of `value` tokens. Once consumed, never refilled. Use for storage budgets.
- **`Concurrency`** — pool of `value` tokens returned when released. Use for limiting parallel connections.

### Enforcement Actions

- **`reject`** — returns `Err(FailedReservation)` with an optional estimated wait time. The agent must handle the error.
- **`throttle`** — Golem suspends the agent until capacity is available. Fully automatic, no code needed.
- **`terminate`** — kills the agent with a failure message.

## 2. Acquire a QuotaToken

Acquire a `QuotaToken` once per resource, typically in the agent constructor:

```rust
use golem_rust::quota::QuotaToken;

let token = QuotaToken::new("api-calls", 1);
```

The second parameter is the **expected amount per reservation**, used for fair scheduling. For simple 1-call = 1-token rate limiting, use `1`.

## 3. Simple Rate Limiting with `with_reservation`

Use `with_reservation` to reserve tokens, run code, and commit actual usage:

```rust
use golem_rust::quota::with_reservation;

let result = with_reservation(&token, 1, |_reservation| {
    let response = call_simple_api();
    (1, response)
});
```

The closure returns `(actual_used, value)`. If `actual_used` < reserved, unused capacity returns to the pool.

## 4. Variable-Cost Reservations (e.g., LLM Tokens)

Reserve the maximum expected cost, then commit actual usage:

```rust
let result = with_reservation(&token, 4000, |_reservation| {
    let response = call_llm(prompt, max_tokens: 4000);
    (response.tokens_used as u64, response)
});
```

## 5. Manual Reserve / Commit

For finer control, use `reserve` and `commit` directly:

```rust
match token.reserve(100) {
    Ok(reservation) => {
        let result = do_work();
        reservation.commit(result.actual_usage);
    }
    Err(e) => {
        log::warn!("Quota unavailable: {e}");
    }
}
```

Dropping a `Reservation` without calling `commit` commits the full reserved amount.

## 6. Splitting Tokens for Agent-to-Agent RPC

Split a portion of your quota to pass to a child agent:

```rust
let child_token: QuotaToken = token.split(200);
let child_agent = SummarizerAgent::new_phantom().await;
let summary = child_agent.summarize(text, child_token).await;
```

The child agent receives the `QuotaToken` as a method parameter and uses it for its own reservations. Merge returned tokens back:

```rust
token.merge(returned_token);
```

`QuotaToken` implements `IntoValue` and `FromValueAndType`, so it can be passed as an agent method parameter.

## 7. Dynamic Resource Updates via CLI

Modify resource limits at runtime — changes affect running agents immediately:

```shell
golem resource update api-calls --limit '{"type":"rate","value":200,"period":"minute","max":2000}' --environment prod
```

## Key Constraints

- Acquire `QuotaToken` once and reuse — do not create a new one per call
- `split` panics if `child_expected_use` exceeds the parent's current expected-use
- `merge` panics if the tokens refer to different resources
- `with_reservation` returns `Err(FailedReservation)` only for `reject` enforcement — `throttle` suspends transparently
- Resource names in code must match the names in `golem.yaml` `resourceDefaults`
