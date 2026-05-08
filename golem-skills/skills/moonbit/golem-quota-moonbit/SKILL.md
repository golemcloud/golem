---
name: golem-quota-moonbit
description: "Adding resource quotas to a MoonBit Golem agent. Use when the user asks about rate limiting, resource quotas, quota tokens, QuotaToken, with_reservation, throttling API calls, limiting concurrency, capacity limits, or splitting tokens between agents."
---

# Adding Resource Quotas to an Agent (MoonBit)

Golem provides a distributed resource quota system via the `@quota` module. Quotas let you define limited resources (API call rates, storage capacity, connection concurrency) and enforce consumption limits across all agents in a deployment.

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

- **`reject`** — returns `Err(FailedReservation)`. The agent must handle the error.
- **`throttle`** — Golem suspends the agent until capacity is available. Fully automatic, no code needed.
- **`terminate`** — kills the agent with a failure message.

## 2. Acquire a QuotaToken

Acquire a `QuotaToken` once per resource, typically in the agent constructor:

```moonbit
let token = @quota.QuotaToken::new("api-calls", 1UL)
```

The second parameter is the **expected amount per reservation** (`UInt64`), used for fair scheduling. For simple 1-call = 1-token rate limiting, use `1UL`.

## 3. Simple Rate Limiting with `with_reservation`

Use `@quota.with_reservation` to reserve tokens, run code, and commit actual usage:

```moonbit
let result = @quota.with_reservation(token, 1UL, fn(reservation) {
  let response = call_simple_api()
  (1UL, response)
})
```

The callback returns `(UInt64, T)` where the first element is actual usage. If actual < reserved, unused capacity returns to the pool.

## 4. Variable-Cost Reservations (e.g., LLM Tokens)

Reserve the maximum expected cost, then commit actual usage:

```moonbit
let result = @quota.with_reservation(token, 4000UL, fn(reservation) {
  let response = call_llm(prompt, max_tokens=4000)
  (response.tokens_used, response)
})
```

## 5. Manual Reserve / Commit

For finer control, use `reserve` and `commit` directly:

```moonbit
match token.reserve(100UL) {
  Ok(reservation) => {
    let result = do_work()
    reservation.commit(result.actual_usage)
  }
  Err(failed) => @log.warn("Quota unavailable")
}
```

## 6. Splitting Tokens for Agent-to-Agent RPC

Split a portion of your quota to pass to a child agent:

```moonbit
let child_token = self.token.split(200UL)
let child_agent = SummarizerAgent::new_phantom()
child_agent.summarize(text, child_token)
```

The child agent receives the `QuotaToken` as a method parameter and uses it for its own reservations. Merge returned tokens back:

```moonbit
token.merge(returned_token)
```

## 7. Dynamic Resource Updates via CLI

Modify resource limits at runtime — changes affect running agents immediately:

```shell
golem resource update api-calls --limit '{"type":"rate","value":200,"period":"minute","max":2000}' --environment prod
```

## Key Constraints

- Acquire `QuotaToken` once and reuse — do not create a new one per call
- All quota amounts are `UInt64` values (use `1UL`, `200UL`, etc.)
- `split` traps if `child_expected_use` exceeds the parent's current expected-use
- `merge` traps if the tokens refer to different resources
- `with_reservation` returns `Result[T, FailedReservation]` — `Err` only for `reject` enforcement; `throttle` suspends transparently
- Resource names in code must match the names in `golem.yaml` `resourceDefaults`
