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

The pinned Effect SDK has these exact semantics:

- `client.get(...)` takes the complete named constructor-parameter record.
- A method with `params: {}` still receives `{}` in remote calls.
- `.schedule(at, input)` expects a WIT wall-clock value with
  `{ seconds: bigint, nanoseconds: number }`.
- `.schedule(...)` is already cancelable and returns `Client.ScheduledInvocation`.
- Cancel with `yield* scheduled.cancel()`. There is no `.scheduleCancelable()` method.
- Dropping or losing the returned handle does not cancel the invocation; only calling `cancel()`
  does.
- Both `client.get(...)` and `.schedule(...)` are lazy Effects and must be yielded.

Construct the WIT datetime explicitly. Convert Effect's `DateTime.Utc` with
`DateTime.toEpochMillis(...)`; the `DateTime` value is not itself the `{ seconds, nanoseconds }`
record required by `.schedule(...)`:

```typescript
const now = yield * DateTime.now;
const atMillis = DateTime.toEpochMillis(now) + delaySeconds * 1_000;
const scheduledAt = {
  seconds: BigInt(Math.floor(atMillis / 1_000)),
  nanoseconds: Math.floor(atMillis % 1_000) * 1_000_000,
};
```

Do not pass `Date.now() + delay`, an ISO string, or `DateTime.toUtc(...)` directly to the typed
client.

## Complete Cancelable Ticker

This example starts immediately, schedules each later tick five seconds ahead, and retains the
live cancellation handle for the next invocation:

```typescript
import { DateTime, Effect, Ref, Schema } from "effect";
import { type Client, defineAgent, method } from "@golemcloud/effect-golem";

const TickerDefinition = defineAgent({
  name: "Ticker",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    start: method({ params: {}, success: Schema.Void }),
    tick: method({ params: {}, success: Schema.Void }),
    cancel: method({ params: {}, success: Schema.Void }),
    getTickCount: method({ params: {}, success: Schema.Number }),
  },
});

export const Ticker = TickerDefinition.implement(({ name }) =>
  Effect.gen(function* () {
    const state = yield* Ref.make({ tickCount: 0, running: false });
    const pending = yield* Ref.make<Client.ScheduledInvocation | null>(null);

    const scheduleNext = (delaySeconds: number) =>
      Effect.gen(function* () {
        const now = yield* DateTime.now;
        const atMillis = DateTime.toEpochMillis(now) + delaySeconds * 1_000;
        const scheduledAt = {
          seconds: BigInt(Math.floor(atMillis / 1_000)),
          nanoseconds: Math.floor(atMillis % 1_000) * 1_000_000,
        };

        const self = yield* TickerDefinition.client.get({ name });
        const scheduled = yield* self.tick.schedule(scheduledAt, {});
        yield* Ref.set(pending, scheduled);
      });

    const tick = Effect.gen(function* () {
      // A handle for an invocation that is already running is no longer useful.
      yield* Ref.set(pending, null);

      const current = yield* Ref.get(state);
      if (!current.running) return;

      const tickCount = current.tickCount + 1;
      yield* Ref.set(state, { ...current, tickCount });

      if (tickCount < 5) {
        yield* scheduleNext(5);
      }
    }).pipe(Effect.orDie);

    return {
      start: () =>
        Effect.gen(function* () {
          const current = yield* Ref.get(state);
          if (current.running) return;
          yield* Ref.set(state, { ...current, running: true });
          yield* tick;
        }),

      tick: () => tick,

      cancel: () =>
        Effect.gen(function* () {
          const current = yield* Ref.get(state);
          yield* Ref.set(state, { ...current, running: false });

          const scheduled = yield* Ref.get(pending);
          yield* Ref.set(pending, null);
          if (scheduled !== null) {
            yield* scheduled.cancel();
          }
        }),

      getTickCount: () =>
        Ref.get(state).pipe(Effect.map(({ tickCount }) => tickCount)),
    };
  }),
);
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

For the simplest recovery-safe stop operation, store a boolean or generation number in normal
agent state:

```typescript
poll: () =>
  Effect.gen(function* () {
    const { running } = yield* Ref.get(state);
    if (!running) return;
    yield* doWork;
    yield* scheduleNext(60);
  }).pipe(Effect.orDie),

stop: () => Ref.update(state, (current) => ({ ...current, running: false })),
```

The already-scheduled invocation still arrives, observes `running === false`, and becomes a no-op.
This is usually enough and requires no cancellation handle.

Use `Client.ScheduledInvocation` only when the pending invocation must be dropped immediately. It
wraps a live host resource and may be kept in an ordinary `Ref`, but it is not schema-serializable:

- do not include it in `Snapshot.define(...)` state;
- the SDK exposes no token ID, decoder, or reopen operation after snapshot restore or component
  recreation;
- keep the durable `running` flag, generation, interval, and next intended time separately;
- design a restored agent so a stale invocation can safely run and exit, rather than promising
  that its old cancellation handle can be recovered.

## Exponential Backoff

Persist the consecutive failure count and derive the next delay after each attempt:

```typescript
const delaySeconds = success
  ? baseIntervalSeconds
  : Math.min(
      baseIntervalSeconds * 2 ** Math.min(consecutiveFailures, 6),
      maxIntervalSeconds,
    );

yield * scheduleNext(delaySeconds);
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

golem agent invocation cancel 'PollerAgent("my-poller")' 'poll-next'
```
