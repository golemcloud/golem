---
name: golem-quota-ts
description: "Adding resource quotas to a TypeScript Golem agent. Use when the user asks about rate limiting, resource quotas, quota tokens, acquireQuotaToken, withReservation, throttling API calls, limiting concurrency, capacity limits, or splitting tokens between agents."
---

# Adding Resource Quotas to an Agent (TypeScript)

Golem provides a distributed resource quota system via `@golemcloud/golem-ts-sdk`. Quotas let you define limited resources (API call rates, storage capacity, connection concurrency) and enforce consumption limits across all agents in a deployment.

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

- **`reject`** — returns an error with an optional estimated wait time. The agent must handle the error.
- **`throttle`** — Golem suspends the agent until capacity is available. Fully automatic, no code needed.
- **`terminate`** — kills the agent with a failure message.

## 2. Acquire a QuotaToken

Acquire a `QuotaToken` once per resource, typically in the agent's `init` — store it on the agent state so handlers can reuse it:

```typescript
import { z } from 'zod';
import { defineAgent, method, acquireQuotaToken } from '@golemcloud/golem-ts-sdk';

export const ApiAgent = defineAgent({
  name: 'ApiAgent',
  id: { name: z.string() },
  methods: {
    call: method({ input: {}, returns: z.string() }),
  },
});

export const ApiAgentImpl = ApiAgent.implement({
  init: () => ({ token: acquireQuotaToken('api-calls', 1n) }),
  methods: {
    async call() {
      // this.token is the QuotaToken acquired in init
      // ...
      return 'ok';
    },
  },
});
```

The second parameter to `acquireQuotaToken` is the **expected amount per reservation** (`bigint`), used for fair scheduling. For simple 1-call = 1-token rate limiting, use `1n`.

## 3. Simple Rate Limiting with `withReservation`

Use `withReservation` to reserve tokens, run code, and commit actual usage:

```typescript
import { withReservation } from "@golemcloud/golem-ts-sdk";

const result = await withReservation(token, 1n, async (reservation) => {
  const response = await callSimpleApi();
  return { used: BigInt(1), value: response };
});
if (result.isErr()) {
  console.warn('Quota unavailable:', result.unwrapErr());
} else {
  const response = result.unwrap();
  // Use the response.
}
```

The callback returns `{ used, value }`. If `used` < reserved, unused capacity returns to the pool.

## 4. Variable-Cost Reservations (e.g., LLM Tokens)

Reserve the maximum expected cost, then commit actual usage:

```typescript
const result = await withReservation(token, 4000n, async (reservation) => {
  const response = await callLlm(prompt, { maxTokens: 4000 });
  return { used: BigInt(response.tokensUsed), value: response };
});
```

## 5. Manual Reserve / Commit

For finer control, use `reserve` and `commit` directly:

`token.reserve(amount)` returns a `Result<Reservation, FailedReservation>`. Inspect it with `.isOk()` / `.isErr()` and read the value with `.unwrap()` / `.unwrapErr()`:

```typescript
const reservationResult = token.reserve(100n);
if (reservationResult.isOk()) {
  const reservation = reservationResult.unwrap();
  const result = doWork();
  reservation.commit(BigInt(result.actualUsage));
} else {
  console.warn("Quota unavailable:", reservationResult.unwrapErr());
}
```

## 6. Splitting a Local Budget

Use a separate parent with sufficient expected-use; the `1n` token above cannot
split off `200n`:

```typescript
import { acquireQuotaToken, withReservation } from '@golemcloud/golem-ts-sdk';

async function useLocalBudget(): Promise<void> {
  const parent = acquireQuotaToken('api-calls', 500n);
  const child = parent.split(200n);
  try {
    const result = await withReservation(child, 1n, async () => {
      return { used: 1n, value: 'done' };
    });
    if (result.isErr()) throw new Error('Quota unavailable');
  } finally {
    parent.merge(child);
  }
}
```

Current RPC limitation: `s.quotaToken()` encodes and decodes the raw WIT quota-token
handle, not the SDK `QuotaToken` wrapper returned by `acquireQuotaToken` or `split`.
Do not pass these wrappers to that marker or assume the receiver gets SDK methods
such as `reserve`, `split`, or `merge`. There is no public wrapper conversion shown
here; keep this budget local rather than casting the wrapper into a raw handle.

## 7. Dynamic Resource Updates via CLI

Modify resource limits at runtime — changes affect running agents immediately:

```shell
golem resource update api-calls --limit '{"type":"Rate","value":200,"period":"minute","max":2000}' --environment prod
```

## Key Constraints

- Acquire `QuotaToken` once and reuse — do not create a new one per call
- All quota amounts are `bigint` values (use `1n`, `200n`, etc.)
- `split` traps if `childExpectedUse` exceeds the parent's current expected-use
- `merge` traps if the tokens refer to different resources
- `withReservation` returns `Result.err` when a reservation is refused; check the result. `throttle` suspends transparently. Callback exceptions or rejections propagate after committing zero usage; handle these separately with `try` / `catch`.
- Resource names in code must match the names in `golem.yaml` `resourceDefaults`
