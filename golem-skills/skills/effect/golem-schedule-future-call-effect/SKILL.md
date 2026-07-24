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
// the agent's constructor parameter:
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
  }).pipe(Effect.orDie);
```

Here `state` is the snapshot-backed `Ref` initialized by the agent implementation. Keep the
application's normal `Ref` or snapshot state pattern rather than replacing it with a Promise or a
local timer. `Effect.orDie` converts typed client and scheduling infrastructure failures into
defects because the example handler declares no public error. If callers should recover from
scheduling failure, map it to a domain error declared in the method's `error` schema instead.

## Exact Call Shape

- `Target.client.get(...)` takes the complete named constructor-parameter record.
- Remote method inputs are named records. Pass `{}` for a method declared with `params: {}`.
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
generated `.scheduleCancelable()` variant, and `.schedule(...)` does not accept a JavaScript
`Date`, an ISO string, or an epoch-millisecond number directly.

## Datetime Representation

The schedule time is an absolute Unix timestamp represented as:

```typescript
const scheduledAt = {
  seconds: 1_700_000_000n,
  nanoseconds: 0,
};
```

`seconds` is a `bigint`. `nanoseconds` is the fractional part of the second as a number from
`0` through `999_999_999`. Convert milliseconds explicitly as shown above; passing
`Date.now() + delayMs` directly has the wrong type and units.

Treat the requested time as a lower bound. Scheduler polling and already queued work on the target
agent can delay execution beyond that instant.

## Cancellation

Every `.schedule(...)` call returns a `Client.ScheduledInvocation`. Keep the returned handle only
when later cancellation is required:

```typescript
const cancelBeforeStart = Effect.gen(function* () {
  const scheduled = yield* recorder.recordCount.schedule(scheduledAt, {
    value,
  });

  // Later, before the invocation starts:
  yield* scheduled.cancel();
});
```

Cancellation is best-effort and becomes a no-op once execution has started. Dropping the handle
does not cancel the schedule. The handle wraps a live host resource and is not schema-serializable:

- it may be retained in an ordinary in-memory `Ref` while the agent instance remains live;
- do not put it in `Snapshot.define(...)` state;
- the pinned SDK exposes no stable schedule id or API for reopening the handle after restoration.

When recovery-safe stopping is required, persist a logical flag or generation number and let a
stale scheduled invocation observe that state and exit without doing work.

## Use Cases

- Schedule the next occurrence of a periodic task after the current run finishes.
- Delay order processing until a cooling-off period expires.
- Deliver reminders or notifications at a chosen wall-clock time.
- Schedule retries with bounded backoff without sleeping inside the current invocation.

Use a scheduled RPC rather than `setTimeout`, `Effect.sleep`, or a forked in-process fiber when the
work must survive suspension and execute as a separate durable agent invocation.
