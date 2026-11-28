---
name: golem-schedule-future-call-effect
description: "Scheduling a future agent invocation from Effect-based Golem agent code. Use for delayed calls, timed agent execution, reminders, or cancelable schedules with @golemcloud/effect-golem."
---

# Scheduling a Future Agent Invocation with Effect

A scheduled invocation asks Golem to call a typed remote agent method at a future wall-clock time.
Submitting the schedule returns immediately after the host accepts it; it does not wait for the
target method to run and does not expose that method's eventual result.

This skill covers agent-to-agent scheduling from Effect code. For scheduling from a shell with
`golem agent invoke --schedule-at`, use the Effect CLI scheduling skill instead.

## Schedule a Typed Remote Method

Obtain the target's typed client, construct the WIT wall-clock datetime, and yield the method's
`.schedule(scheduledAt, input)` Effect:

```typescript
import { Effect, Ref } from "effect";
import { DelayedRecorderDefinition } from "./delayed-recorder.js";

// In an implementation where `state` is the snapshot Ref and `name` is
// the agent's agent id field:
const scheduleReport = () =>
  Effect.gen(function* () {
    const { count: newCount } = yield* Ref.updateAndGet(state, ({ count }) => ({
      count: count + 1,
    }));

    const atMillis = Date.now() + 30_000;
    const scheduledAt = {
      seconds: BigInt(Math.floor(atMillis / 1_000)),
      nanoseconds: Math.floor(atMillis % 1_000) * 1_000_000,
    };

    const recorder = yield* DelayedRecorderDefinition.client.upsert({ name });
    yield* recorder.recordCount.schedule(scheduledAt, { value: newCount });
    return newCount;
  }).pipe(Effect.scoped, Effect.orDie);
```

Here `state` is the snapshot-backed `Ref` initialized by the agent implementation. Keep the
application's normal `Ref` or snapshot state pattern rather than replacing it with a Promise or a
local timer. `Effect.orDie` converts typed client and scheduling infrastructure failures into
defects because the example handler declares no public error. If callers should recover from
scheduling failure, map it to a domain error declared in the method's `error` schema instead.

## Exact Call Shape

- `Target.client.upsert(...)` takes the complete named agent `id` record.
- Remote method inputs are named records. Pass `{}` for a method declared with `input: {}`.
- `.schedule(...)` takes the absolute datetime first and the method input second.
- Both `client.upsert(...)` and `.schedule(...)` are lazy Effects and must be yielded.
- Yielding `.schedule(...)` waits only until the target accepted the invocation, not for future
  execution. It requires a `Scope`, which owns the returned handle; wrap the Effect in
  `Effect.scoped` as shown.
- Scheduling reports only submission failures. The target method's later success or typed failure
  is reported only by the handle's `awaitResult`.

For a no-parameter method:

```typescript
const scheduleIncrement = Effect.gen(function* () {
  const counter = yield* CounterDefinition.client.upsert({ name: "my-counter" });
  yield* counter.increment.schedule(scheduledAt, {});
});
```

Do not translate the Promise-based TypeScript SDK mechanically: the handle's `status`,
`awaitResult` and `cancel` are Effects, not methods. `.schedule(...)` accepts `Datetime.Input`: a WIT
datetime, a JavaScript `Date`, an Effect `DateTime`, or a numeric epoch-millisecond timestamp.
ISO strings are not supported. Invalid or out-of-range values fail during normalization.

## Datetime Representation

The schedule time is an absolute Unix timestamp. For nanosecond precision, use the WIT form:

```typescript
const scheduledAt = {
  seconds: 1_700_000_000n,
  nanoseconds: 0,
};
```

`seconds` is a `bigint`. `nanoseconds` is the fractional part of the second as a number from
`0` through `999_999_999`. Convert milliseconds explicitly as shown above when constructing the
WIT record, or pass `Date.now() + delayMs` directly as a numeric epoch-millisecond input.

Treat the requested time as a lower bound. Scheduler polling and already queued work on the target
agent can delay execution beyond that instant.

## Cancellation

Every `.schedule(...)` call succeeds with an `InvocationId.Invocation` handle owned by the current
Scope. Its `cancel` Effect withdraws the invocation while it has not started:

```typescript
const cancelBeforeStart = Effect.gen(function* () {
  const scheduled = yield* recorder.recordCount.schedule(scheduledAt, {
    value,
  });

  // Before execution starts, within this same open Scope:
  const withdrawn = yield* scheduled.cancel;
}).pipe(Effect.scoped);
```

`cancel` succeeds with `true` when it withdrew the invocation, `false` when it already finished or
was already cancelled, and fails with `RpcCannotCancelRunning` when it is running. Awaiting a
cancelled invocation with `scheduled.awaitResult` fails with `RpcCancelled`. Closing the Scope, or
dropping the handle, does not cancel the schedule.

The handle wraps a live host resource and is not schema-serializable. To cancel from a later
invocation, store its ID as text and rebuild the handle without a remote call:

```typescript
import { InvocationId } from "@golemcloud/effect-golem";

// When scheduling: keep the text in snapshot state.
const pendingText = InvocationId.format(scheduled.id);

// In a later invocation, inside a scoped Effect:
const id = yield* InvocationId.parse(pendingText);
const handle = yield* InvocationId.get(id, DelayedRecorderDefinition.methods.recordCount);
yield* handle.cancel;
```

The ID only locates the invocation; it is not authority. Only the agent that scheduled it, or a
caller with management permission, can observe or cancel it. `.schedule(...)` never sends a scope
card and rejects methods with streams. For a recurring loop, combine cancellation with a persisted
generation number so stale invocations exit without doing work; see
[`golem-recurring-task-effect`](../golem-recurring-task-effect/SKILL.md).

## Use Cases

- Schedule the next occurrence of a periodic task after the current run finishes.
- Delay order processing until a cooling-off period expires.
- Deliver reminders or notifications at a chosen wall-clock time.
- Schedule retries with bounded backoff without sleeping inside the current invocation.

Use a scheduled RPC rather than `setTimeout`, `Effect.sleep`, or a forked in-process fiber when the
work must survive suspension and execute as a separate durable agent invocation.
