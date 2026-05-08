---
name: golem-quota-scala
description: "Adding resource quotas to a Scala Golem agent. Use when the user asks about rate limiting, resource quotas, quota tokens, QuotaToken, withReservation, throttling API calls, limiting concurrency, capacity limits, or splitting tokens between agents."
---

# Adding Resource Quotas to an Agent (Scala)

Golem provides a distributed resource quota system via `golem.host.QuotaApi`. Quotas let you define limited resources (API call rates, storage capacity, connection concurrency) and enforce consumption limits across all agents in a deployment.

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

- **`reject`** — returns `Left(FailedReservation)` with an optional estimated wait time. The agent must handle the error.
- **`throttle`** — Golem suspends the agent until capacity is available. Fully automatic, no code needed.
- **`terminate`** — kills the agent with a failure message.

## 2. Acquire a QuotaToken

Acquire a `QuotaToken` once per resource, typically in the agent constructor:

```scala
import golem.host.QuotaApi._

val token = QuotaToken("api-calls", BigInt(1))
```

The second parameter is the **expected amount per reservation**, used for fair scheduling. For simple 1-call = 1-token rate limiting, use `BigInt(1)`.

## 3. Simple Rate Limiting with `withReservation`

Use `withReservation` to reserve tokens, run code, and commit actual usage:

```scala
val result = withReservation(token, BigInt(1)) { reservation =>
  callSimpleApi().map { response =>
    (BigInt(1), response)
  }
}
```

The callback returns `Future[(BigInt, T)]` where the first element is actual usage. If actual < reserved, unused capacity returns to the pool.

## 4. Variable-Cost Reservations (e.g., LLM Tokens)

Reserve the maximum expected cost, then commit actual usage:

```scala
val result = withReservation(token, BigInt(4000)) { reservation =>
  callLlm(prompt, maxTokens = 4000).map { response =>
    (BigInt(response.tokensUsed), response)
  }
}
```

## 5. Manual Reserve / Commit

For finer control, use `reserve` and `commit` directly:

```scala
token.reserve(BigInt(100)) match {
  case Right(reservation) =>
    val result = doWork()
    reservation.commit(BigInt(result.actualUsage))
  case Left(failed) =>
    println(s"Quota unavailable: $failed")
}
```

## 6. Splitting Tokens for Agent-to-Agent RPC

Split a portion of your quota to pass to a child agent:

```scala
val childToken: QuotaToken = token.split(BigInt(200))
for {
  childAgent <- SummarizerAgent.newPhantom()
  summary <- childAgent.summarize(text, childToken)
} yield summary
```

The child agent receives the `QuotaToken` as a method parameter and uses it for its own reservations. Merge returned tokens back:

```scala
token.merge(returnedToken)
```

## 7. Dynamic Resource Updates via CLI

Modify resource limits at runtime — changes affect running agents immediately:

```shell
golem resource update api-calls --limit '{"type":"rate","value":200,"period":"minute","max":2000}' --environment prod
```

## Key Constraints

- Acquire `QuotaToken` once and reuse — do not create a new one per call
- All quota amounts are `BigInt` values
- `split` traps if `childExpectedUse` exceeds the parent's current expected-use
- `merge` traps if the tokens refer to different resources
- `withReservation` returns `Future[Either[FailedReservation, T]]` — `Left` only for `reject` enforcement; `throttle` suspends transparently
- Resource names in code must match the names in `golem.yaml` `resourceDefaults`
