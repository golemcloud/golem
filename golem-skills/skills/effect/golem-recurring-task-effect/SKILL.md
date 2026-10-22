---
name: golem-recurring-task-effect
description: "Implementing durable recurring, cron-like, polling, cleanup, and heartbeat tasks by self-scheduling Effect Golem agent methods. Use for periodic jobs, exponential backoff, cancelable schedules, or self-scheduling agents with @golemcloud/effect-golem."
---

# Recurring Tasks via Self-Scheduling in Effect

An Effect Golem agent can schedule a future invocation of one of its own methods after each run.
The scheduled invocation is durable and survives agent suspension or recovery; the agent does not
need an in-process timer, sleeping fiber, or external cron service.

## The Scheduling API

Obtain a typed client for the same durable identity, then yield the remote method's
`.schedule(scheduledAt, input)` Effect:

```typescript
const self = yield * PollerDefinition.client.get({ name });
const scheduled = yield * self.poll.schedule(scheduledAt, {});
```

Use these client conventions:

- `client.get(...)` takes the complete named constructor-parameter record.
- A method with `input: {}` still receives `{}` in remote calls.
- `.schedule(at, input)` accepts `Datetime.Input`: a WIT wall-clock value with
  `{ seconds: bigint, nanoseconds: number }`, a JavaScript `Date`, an Effect `DateTime`, or
  a numeric epoch-millisecond timestamp.
- `.schedule(...)` is already cancelable and returns `Client.ScheduledInvocation`.
- Cancel with `yield* scheduled.cancel()`. There is no `.scheduleCancelable()` method.
- The handle belongs to the allocating Scope. Closing that Scope drops the host cancellation
  resource, but does not cancel the invocation. A `Ref` does not extend that lifetime.
- Both `client.get(...)` and `.schedule(...)` are lazy Effects and must be yielded.

The client normalizes supported inputs. To construct a WIT datetime explicitly from an Effect
`DateTime.Utc`, convert it with `DateTime.toEpochMillis(...)`:

```typescript
const now = yield * DateTime.now;
const atMillis = DateTime.toEpochMillis(now) + delaySeconds * 1_000;
const scheduledAt = {
  seconds: BigInt(Math.floor(atMillis / 1_000)),
  nanoseconds: Math.floor(atMillis % 1_000) * 1_000_000,
};
```

Alternatively, pass `atMillis` directly to `.schedule(atMillis, input)`. JavaScript `Date` and
Effect `DateTime` values are also supported. ISO strings are not. Invalid or out-of-range inputs
fail during normalization.

## Complete Logically Cancelable Ticker

This example runs five ticks, starting immediately and scheduling each later tick five seconds
ahead. Repeated `start` calls while running are no-ops; after cancellation or completion, `start`
begins a fresh run. Each run has a new generation so stale ticks cannot do work or create a second
chain. Like the TypeScript recurring-task recipe, it stores only logical state:

```typescript
import { DateTime, Effect, Ref, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

const TickerDefinition = defineAgent({
  name: "Ticker",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  methods: {
    start: method({ input: {}, success: Schema.Void }),
    tick: method({ input: { generation: Schema.Number }, success: Schema.Void }),
    cancel: method({ input: {}, success: Schema.Void }),
    getTickCount: method({ input: {}, success: Schema.Number }),
  },
});

export const Ticker = TickerDefinition.implement({
  init: ({ name }) =>
    Effect.gen(function* () {
      const state = yield* Ref.make({ tickCount: 0, running: false, generation: 0 });
      return { name, state };
    }),
  methods: ({ name, state }) => {
    const scheduleNext = (delaySeconds: number, generation: number) =>
      Effect.gen(function* () {
        const now = yield* DateTime.now;
        const atMillis = DateTime.toEpochMillis(now) + delaySeconds * 1_000;
        const scheduledAt = {
          seconds: BigInt(Math.floor(atMillis / 1_000)),
          nanoseconds: Math.floor(atMillis % 1_000) * 1_000_000,
        };

        const self = yield* TickerDefinition.client.get({ name });
        yield* self.tick.schedule(scheduledAt, { generation });
      }).pipe(Effect.scoped);

    const tick = (generation: number) => Effect.gen(function* () {
      const current = yield* Ref.get(state);
      if (!current.running || current.generation !== generation) return;

      const tickCount = current.tickCount + 1;
      yield* Ref.set(state, { ...current, tickCount, running: tickCount < 5 });

      if (tickCount < 5) {
        yield* scheduleNext(5, generation);
      }
    }).pipe(Effect.orDie);

    return {
      start: () =>
        Effect.gen(function* () {
          const current = yield* Ref.get(state);
          if (current.running) return;
          const generation = current.generation + 1;
          yield* Ref.set(state, { tickCount: 0, running: true, generation });
          yield* tick(generation);
        }),

      tick: ({ generation }) => tick(generation),

      cancel: () =>
        Ref.update(state, (current) => ({
          ...current, running: false, generation: current.generation + 1,
        })),

      getTickCount: () =>
        Ref.get(state).pipe(Effect.map(({ tickCount }) => tickCount)),
    };
  },
});
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./ticker.js";
```

`client.get(...)` and `.schedule(...)` can fail with SDK infrastructure errors. The example uses
`Effect.orDie` around the recurring tick because its public method declares no typed error. If
callers should handle scheduling failures, map them to a domain error declared in the method's
`error` schema instead.

## Logical Stop Versus Exact Cancellation

An already-scheduled invocation still arrives, checks its generation, and becomes a no-op after
cancel or restart. A boolean alone is insufficient for a restartable loop: an old tick could see
`running === true` after restart and create a second chain. Durable replay reconstructs the logical
state. If adding snapshots, save and restore the generation, running flag, and tick count together;
never reset the generation while old schedules can still arrive.

`Client.ScheduledInvocation` is a live scoped host resource, not schema-serializable state. Call
`cancel()` only while its allocating Scope is still open. Do not save it in a `Ref` for a later
invocation after `Effect.scoped` has closed. Logical generation cancellation is the recommended
workflow for later invocations; it requires no host cancellation handle.

## Exponential Backoff

Persist the consecutive failure count and derive the next delay after each attempt:

```typescript
const delaySeconds = success
  ? baseIntervalSeconds
  : Math.min(
      baseIntervalSeconds * 2 ** Math.min(consecutiveFailures, 6),
      maxIntervalSeconds,
    );

yield * scheduleNext(delaySeconds, generation);
```

Reset `consecutiveFailures` to zero on success and increment it before computing a failed
attempt's delay. Keep the cap so an unavailable dependency does not make the recurrence disappear
for an unexpectedly long time.

## Operational Constraints

- Agent invocations are sequential, so two methods on the same agent do not mutate recurrence
  state concurrently.
- Keep the scheduled method idempotent because durable recovery can retry an invocation.
- Schedule the next occurrence after the current work finishes. Scheduling first can create
  overlapping intent if the current work fails and retries.
- Treat the scheduled datetime as a lower bound, not an exact timer. The host scheduler polls for
  due work, and the invocation may also wait behind queued work on the same agent. Tests should
  poll observable state or allow scheduler latency beyond the requested delay.
- Repeated `start` calls must not create parallel scheduling chains; check the running state first.
- Each tick grows the durable agent's oplog. For frequent or long-running recurrences, use
  [`golem-custom-snapshot-effect`](../golem-custom-snapshot-effect/SKILL.md) and snapshot only
  schema-serializable logical state.
- Use Effect HTTP route metadata when a poll count or status must be observable over HTTP; the
  self-scheduling client API remains the same.

## CLI Scheduling and Cancellation

CLI method names and values use TypeScript casing and syntax for Effect applications. An explicit
idempotency key can identify a CLI-scheduled invocation for later cancellation:

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' \
  'PollerAgent("my-poller")' poll

golem agent cancel-invocation 'PollerAgent("my-poller")' 'poll-next'
```
