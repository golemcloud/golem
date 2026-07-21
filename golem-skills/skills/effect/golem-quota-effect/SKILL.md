---
name: golem-quota-effect
description: "Adding resource quotas to an Effect-based Golem agent. Use when the user asks about rate limiting, resource quotas, Quota tokens, Quota.acquireQuotaToken, Quota.withReservation, throttling API calls, limiting concurrency, capacity limits, or splitting tokens between agents."
---

# Adding Resource Quotas to an Effect Golem Agent

Golem provides distributed resource quotas through the `Quota` namespace from
`@golemcloud/effect-golem`. The manifest defines each resource's policy; agent code acquires a
token for that resource and performs Effect-based reservations against it.

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
        value: 1073741824 # 1 GB
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

- **`Rate`** refills `value` tokens every `period` (`second`, `minute`, `hour`, or `day`),
  capped at `max`. Use it for API-call rates.
- **`Capacity`** is a fixed pool of `value` tokens. Used capacity does not refill. Use it for
  storage or budget limits.
- **`Concurrency`** is a pool of `value` tokens returned when released. Use it to bound parallel
  connections or jobs.

### Enforcement Actions

- **`reject`** makes `Quota.reserve` or `Quota.withReservation` fail with a typed
  `Quota.FailedReservationError`, whose `estimatedWaitNanos` is `bigint | undefined`.
- **`throttle`** suspends the agent in the host until capacity is available.
- **`terminate`** terminates the offending agent in the host.

The resource name passed from code must exactly match the manifest key. Do not recreate limit or
enforcement policy in the Effect implementation.

## 2. Acquire and Reuse a Token

Import Effect APIs from `effect` and quota APIs through the SDK's `Quota` namespace:

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, method, Quota } from "@golemcloud/effect-golem";
```

`Quota.acquireQuotaToken` is an Effect. Acquire it once in the agent's `.implement(...)` Effect
body and close over it in the returned handlers. This closure is the Effect equivalent of keeping
the token in the constructed agent's state; do not acquire a new token for every invocation or at
module top level.

```typescript
const QuotaCallResult = Schema.Struct({
  input: Schema.String,
  success: Schema.Boolean,
});

export const QuotaAgent = defineAgent({
  name: "QuotaAgent",
  mode: "durable",
  constructorParams: { instanceName: Schema.String },
  methods: {
    rateLimitedCall: method({
      params: { input: Schema.String },
      success: QuotaCallResult,
    }),
  },
}).implement(() =>
  Effect.gen(function* () {
    const token = yield* Quota.acquireQuotaToken("api-calls", 1n);

    return {
      rateLimitedCall: ({ input }) =>
        Quota.withReservation(token, 1n, () =>
          Effect.succeed({
            used: 1n,
            value: { input, success: true },
          }),
        ).pipe(
          Effect.catchTag("FailedReservationError", () =>
            Effect.succeed({ input, success: false }),
          ),
          Effect.orDie,
        ),
    };
  }),
);
```

The second acquisition argument is the expected amount per reservation and is used for fair
scheduling. All quota amounts are `bigint` values. In the example, `Effect.catchTag` recovers only
the normal `reject` outcome; `Effect.orDie` converts the remaining unexpected `QuotaHostError` into
an invocation failure instead of incorrectly reporting quota exhaustion. If callers should
recover from host failures, map them to a domain error declared by the method instead.

## 3. Reserve Fixed or Variable Costs

`Quota.withReservation` reserves an amount, runs an Effect, commits the reported usage, and returns
the callback's `value`:

```typescript
const response = yield* Quota.withReservation(token, 1n, () =>
  callSimpleApi().pipe(
    Effect.map((value) => ({
      used: 1n,
      value,
    })),
  ),
);
```

For variable-cost work, reserve the maximum expected cost and report actual usage:

```typescript
const response = yield* Quota.withReservation(token, 4_000n, () =>
  Effect.gen(function* () {
    const result = yield* callLlm(prompt, { maxTokens: 4_000 });
    return {
      used: BigInt(result.tokensUsed),
      value: result,
    };
  }),
);
```

The callback must return an `Effect` whose success value is `{ used: bigint, value: A }`.
`Quota.withReservation` already manages its own scope; do not add a redundant `Effect.scoped`
around it. If the callback fails, dies, or is interrupted, the reservation finalizer commits `0n`
and preserves the original cause.

## 4. Reserve and Commit Manually

Use `Quota.reserve` and `Quota.commit` for finer control. A manual reservation requires a Scope, so
wrap the complete lifetime in `Effect.scoped`:

```typescript
const result = yield* Effect.scoped(
  Effect.gen(function* () {
    const reservation = yield* Quota.reserve(token, 100n);
    const result = yield* doWork();
    yield* Quota.commit(reservation, BigInt(result.actualUsage));
    return result;
  }),
);
```

Committing less than the reserved amount returns unused capacity. Committing more records debt
against the token. If the scope closes before an explicit commit, the SDK performs a best-effort
`commit(0n)`; a successful explicit commit makes that finalizer a no-op.

## 5. Split Tokens Across Agent RPC

Use `Quota.QuotaToken` as the Effect Schema for quota-token method parameters and results. The SDK
codec converts between the live token and its wire record automatically:

```typescript
const SummaryResult = Schema.Struct({
  summary: Schema.String,
  token: Quota.QuotaToken,
});

const summarize = method({
  params: {
    text: Schema.String,
    token: Quota.QuotaToken,
  },
  success: SummaryResult,
});
```

Split from the parent, send the child token through an existing typed agent client, and merge the
returned token:

```typescript
const childToken = yield* Quota.split(token, 200n);
const summarizer = yield* SummarizerAgent.client.get({ name: "sum-1" });
const { summary, token: returnedToken } = yield* summarizer.summarize({
  text,
  token: childToken,
});
yield* Quota.merge(token, returnedToken);
return summary;
```

`Quota.split` fails with `QuotaHostError` if the child expected use exceeds the parent's current
expected use. `Quota.merge` requires tokens from the same resource and environment. A successful
merge consumes the returned child token, so do not use it afterward.

## 6. Update a Deployed Resource

Resource updates affect running agents without changing their Effect code:

```shell
golem resource update api-calls --limit '{"type":"rate","value":200,"period":"minute","max":2000}' --environment prod
```

## Key Constraints

- Import `Quota` from `@golemcloud/effect-golem`; there are no flat public quota functions and the
  host quota client is internal.
- Acquire each shared token once in the agent implementation Effect and reuse it.
- Use `bigint` amounts such as `1n`, `200n`, and `4_000n`.
- Use `Quota.withReservation` for automatic lifetime management and `Effect.scoped` for manual
  `Quota.reserve` / `Quota.commit` lifetimes.
- Recover a rejected reservation with `Effect.catchTag("FailedReservationError", ...)`; do not use
  a broad catch that also hides body failures or `QuotaHostError`.
- Resource names in code must exactly match their `golem.yaml` `resourceDefaults` keys.
- Use `Quota.QuotaToken`, not `Quota.QuotaTokenRecord`, in agent RPC schemas. The record is the
  inspectable wire representation, not the live token handle.
