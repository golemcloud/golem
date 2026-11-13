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

    const recorder = yield* DelayedRecorderDefinition.client.get({ name });
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

- `Target.client.get(...)` takes the complete named agent `id` record.
- Remote method inputs are named records. Pass `{}` for a method declared with `input: {}`.
- `.schedule(...)` takes the absolute datetime first and the method input second.
- Both `client.get(...)` and `.schedule(...)` are lazy Effects and must be yielded.
- Yielding `.schedule(...)` waits only for schedule submission, not for future execution.
- Scheduling reports only submission failures. The target method's later success or typed failure
  is not returned to the caller.

For a no-parameter method:

```typescript
const scheduleIncrement = Effect.gen(function* () {
  const counter = yield* CounterDefinition.client.get({ name: "my-counter" });
  yield* counter.increment.schedule(scheduledAt, {});
});
```

Do not translate the Promise-based TypeScript SDK mechanically. In this Effect SDK there is no
generated `.scheduleCancelable()` variant. `.schedule(...)` accepts `Datetime.Input`: a WIT
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

Every `.schedule(...)` call returns a `Client.ScheduledInvocation` owned by its allocating Scope.
Use it for cancellation only while that Scope remains open:

```typescript
const cancelBeforeStart = Effect.gen(function* () {
  const scheduled = yield* recorder.recordCount.schedule(scheduledAt, {
    value,
  });

  // Before execution starts, within this same open Scope:
  yield* scheduled.cancel();
}).pipe(Effect.scoped);
```

Cancellation is best-effort and becomes a no-op once execution has started. Dropping the handle
does not cancel the schedule. The handle wraps a live host resource and is not schema-serializable:

- closing the allocating Scope drops the cancellation resource; a `Ref` does not extend its
  lifetime, and a handle saved past `Effect.scoped` cannot be used by a later invocation;
- do not put it in `Snapshot.define(...)` state;
- the pinned SDK exposes no stable schedule id or API for reopening the handle after restoration.

For later cancellation, persist a generation number, include it in the scheduled method input,
and let stale invocations exit without doing work. Increment the generation on cancellation and
restart; a boolean alone allows an old invocation to resume work after a restart. See the complete
workflow in [`golem-recurring-task-effect`](../golem-recurring-task-effect/SKILL.md).

## Use Cases

- Schedule the next occurrence of a periodic task after the current run finishes.
- Delay order processing until a cooling-off period expires.
- Deliver reminders or notifications at a chosen wall-clock time.
- Schedule retries with bounded backoff without sleeping inside the current invocation.

Use a scheduled RPC rather than `setTimeout`, `Effect.sleep`, or a forked in-process fiber when the
work must survive suspension and execute as a separate durable agent invocation.
