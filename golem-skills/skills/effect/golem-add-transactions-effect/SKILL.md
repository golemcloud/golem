---
name: golem-add-transactions-effect
description: "Adding Effect-based saga transactions with compensating rollback to a Golem agent. Use when an @golemcloud/effect-golem project needs fallible multi-step operations, reverse-order compensation, rollback reporting, or durable saga recovery."
---

# Saga-Pattern Transactions in Effect Golem Agents

Use the `Saga` namespace from `@golemcloud/effect-golem`. A saga is an ordinary Effect program:
each successful step registers an Effect that can compensate it, and the transaction wrapper runs
registered compensations in reverse order when the body fails.

```typescript
import { Cause, Effect, Exit, Schema } from "effect";
import { method, Saga } from "@golemcloud/effect-golem";
```

Do not import transaction helpers from `@golemcloud/golem-ts-sdk`; its Promise/`Result` API is not
the Effect SDK API.

## Choose the Compensation API

The Effect SDK provides three ways to define a step:

- `Saga.withCompensation(effect, compensate)` registers an infallible compensation whose error
  channel is `never`.
- `Saga.withFallibleCompensation(effect, compensate)` allows compensation to fail. A failed
  rollback becomes `FailedAndRolledBackPartially`; remaining compensations still run.
- `Saga.operation({ execute, compensate })` creates a reusable `(input) => Effect` step. The
  compensation receives `(input, output, cause)`, but it must be infallible.

Use `withFallibleCompensation` for remote calls because a remote compensation has a typed
`Client.RemoteCallError`. Do not hide that error merely to satisfy `Saga.operation`'s `never`
requirement.

Assuming these application functions already return Effects, a reusable infallible operation looks
like this:

```typescript
const reserveInventory = Saga.operation({
  execute: (sku: string) => reserve(sku),
  compensate: (_sku, reservation, _cause) =>
    release(reservation.reservationId),
});
```

The compensation is registered only after `execute` succeeds. A failing execute path therefore
does not compensate itself.

## Fallible Transactions

Wrap the complete sequence in `Saga.fallibleTransaction`. Signal expected failures that should
trigger rollback with `Effect.fail`, not `throw`, `Effect.die`, or `Effect.dieMessage`:

```typescript
const transaction = Saga.fallibleTransaction(
  Effect.gen(function* () {
    const reservation = yield* reserveInventory("SKU-123");
    const charge = yield* chargePayment(4999);

    if (mustRollback) {
      return yield* Effect.fail("requested-rollback" as const);
    }

    return { reservation, charge };
  }),
);
```

A typed failure returns one of these through the Effect error channel:

```typescript
type TransactionFailure<E> =
  | { readonly _tag: "FailedAndRolledBackCompletely"; readonly error: E }
  | {
      readonly _tag: "FailedAndRolledBackPartially";
      readonly error: E;
      readonly compensationError: unknown;
    };
```

`fallibleTransaction` can also fail with SDK host errors or `NestedSagaError`. If an agent method
returns a business-result DTO, convert only the two transaction-failure tags into that DTO. Do not
misreport unexpected host failures as a successful rollback.

## Agent-to-Agent Ledger Pattern

The pinned Effect component does not provide an outgoing HTTP client or a global `fetch`
implementation. When a saga step targets another Golem agent, use the supported typed agent client.
The same target methods can also have `Http` endpoint metadata, so external HTTP verification sees
the state mutated by agent-to-agent calls.

Given an implemented `OrderLedger` definition with `reserve`, `release`, `charge`, and `refund`
methods, this handler performs two steps and reports rollback as a `ProcessOrderResult`:

```typescript
const ProcessOrderInputSchema = Schema.Struct({
  orderId: Schema.String,
  failAfterCharge: Schema.Boolean,
});

const ProcessOrderResultSchema = Schema.Struct({
  success: Schema.Boolean,
  error: Schema.NullOr(Schema.String),
});

type ProcessOrderInput = typeof ProcessOrderInputSchema.Type;
type ProcessOrderResult = typeof ProcessOrderResultSchema.Type;

// Add this entry to the agent's methods record.
const processOrderMethod = method({
  params: { input: ProcessOrderInputSchema },
  success: ProcessOrderResultSchema,
});

const processOrder = ({ orderId, failAfterCharge }: ProcessOrderInput) =>
  Effect.gen(function* () {
    const ledger = yield* OrderLedger.client.get({ orderId });

    const exit = yield* Effect.exit(
      Saga.fallibleTransaction(
        Effect.gen(function* () {
          yield* Saga.withFallibleCompensation(
            ledger.reserve({}),
            () => ledger.release({}),
          );

          yield* Saga.withFallibleCompensation(
            ledger.charge({}),
            () => ledger.refund({}),
          );

          if (failAfterCharge) {
            return yield* Effect.fail("requested-rollback" as const);
          }

          return "committed" as const;
        }),
      ),
    );

    if (Exit.isSuccess(exit)) {
      return { success: true, error: null } satisfies ProcessOrderResult;
    }

    const failReason = exit.cause.reasons.find(Cause.isFailReason);
    const failure = failReason?.error;

    if (
      typeof failure === "object" &&
      failure !== null &&
      "_tag" in failure &&
      (failure._tag === "FailedAndRolledBackCompletely" ||
        failure._tag === "FailedAndRolledBackPartially")
    ) {
      const rollback = failure as Saga.TransactionFailure<unknown>;
      const error =
        rollback._tag === "FailedAndRolledBackPartially"
          ? `${String(rollback.error)}; compensation: ${String(rollback.compensationError)}`
          : String(rollback.error);
      return { success: false, error } satisfies ProcessOrderResult;
    }

    return yield* Effect.die(exit.cause);
  });
```

Put `processOrder: processOrderMethod` in the agent definition's `methods` record. The one
top-level parameter named `input` is intentional: it lets callers provide the two fields as one
TypeScript record value. The implementation receives the named-parameter wrapper, so expose the
handler as follows:

```typescript
processOrder: ({ input }) => processOrder(input),
```

The handler must return that Effect, and every successful branch of the Effect must return a
`ProcessOrderResult`. Do not declare `success: Schema.Void`, drop the result with `Effect.tap`, or
yield the mapping Effect and then fall through. A void method omits `result_json` from CLI JSON
output.

The CLI accepts one shell positional per top-level method parameter. With the single `input` record
above, invoke it with one record positional:

```shell
golem agent invoke 'CounterAgent("main")' processOrder \
  '{ orderId: "order-1", failAfterCharge: true }'
```

If instead a method declares `params: { orderId: ..., failAfterCharge: ... }`, pass two separate
shell positionals. Do not combine two top-level parameters into one comma-separated argument.

For rollback order `refund` then `release`, register the reserve step first and the charge step
second. Define the ledger's DELETE HTTP routes with `Http.del(...)`, but invoke the same underlying
methods through `OrderLedger.client` from the saga; do not try to fetch the ledger's HTTP URL from
inside the Effect component.

## Infallible Transactions

`Saga.infallibleTransaction(body)` accepts only `Effect<A, never, R>`. On interruption it drains
compensations, rewinds to the transaction checkpoint with the Golem oplog, and relies on durable
replay to try again. Do not cast away a real error channel or turn expected failures into defects to
force a body into this API. A deterministic retry condition can loop forever, and rewind recovery
must not be wrapped in `persistNothing`.

Use `fallibleTransaction` whenever failure is an acceptable caller-visible outcome. It is the right
choice for methods returning `{ success, error }`.

## Failure and Durability Rules

- Keep execute and compensation actions idempotent; durable replay or interruption can repeat work.
- Compensations run sequentially in reverse registration order.
- Await every step by yielding its Effect. Starting work outside the returned Effect breaks ordering.
- Defects bypass compensation and trap the worker. Use the typed error channel for business failure.
- Do not nest `fallibleTransaction` or `infallibleTransaction`; nested sagas fail with
  `NestedSagaError`.
- Do not use compensation combinators outside a transaction; the step runs but no rollback is
  retained.
- Saga atomic regions make each durable step recoverable; they do not make external systems ACID.
- Use normal awaited agent-client calls in steps, not `.trigger(...)` or scheduled calls.
- Keep the agent durable and use the default durable persistence level for oplog recovery.

## Authoritative API Sources

- [`Saga` implementation and signatures](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Saga.ts)
- [Pinned booking-saga agent example](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/integration-test/components/agents/src/booking-saga-agent.ts)
- [Typed agent client API](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Client.ts)
- [Method schemas and handler input](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/internal/method.ts)
- [Declarative HTTP metadata](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Http.ts)
- [Pinned component world showing available host imports](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/wit/main.wit)
