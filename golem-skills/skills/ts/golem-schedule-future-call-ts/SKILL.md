---
name: golem-schedule-future-call-ts
description: "Scheduling a future agent invocation in a TypeScript Golem project. Use when the user asks about delayed invocations, scheduling calls for later, or timed agent execution."
---

# Scheduling a Future Agent Invocation (TypeScript)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call resolves once the target has durably accepted the invocation; the target agent processes it when the scheduled time arrives.

## Usage

Every method on a definition RPC client has a `.schedule()` variant that
takes a `Datetime` as the first argument, followed by the method's input record
(omit the input for methods declared with `input: {}`). It returns
`Promise<Invocation<T>>`, the handle of the accepted invocation. Await it: a
rejection means the invocation was not scheduled. Dropping the handle does not
cancel the invocation.

```typescript
import { Datetime } from '@golemcloud/golem-ts-sdk';
import { Counter } from './counter-agent.js';

const counter = Counter.client.upsert({ name: 'my-counter' });

// Schedule increment to run 60 seconds from now.
const inOneMinute = Datetime.fromDate(new Date(Date.now() + 60_000));
await counter.increment.schedule(inOneMinute);

// Schedule with arguments.
const reporter = ReportAgent.client.upsert({ name: 'daily' });
await reporter.generateReport.schedule(
    Datetime.parse('2026-01-02T00:00:00Z'),
    { kind: 'summary' },
);
```

## Datetime Type

The `Datetime` argument is the SDK `Datetime` type: a plain
`{ seconds: bigint, nanoseconds: number }` object with seconds since the Unix
epoch in UTC. Import it from the SDK and build values with its helpers:

```typescript
import { Datetime } from '@golemcloud/golem-ts-sdk';

const fromDate = Datetime.fromDate(new Date(Date.now() + 5 * 60_000));
const fromText = Datetime.parse('2026-01-02T09:30:00+02:00'); // any RFC 3339 offset, stored as UTC
const inline: Datetime = { seconds: 1700000000n, nanoseconds: 0 };

Datetime.toISOString(fromText); // '2026-01-02T07:30:00Z'
Datetime.toDate(inline);        // a JavaScript Date (millisecond precision)
```

## Cancellation

Keep the `Invocation` returned by `.schedule(at, input)` and call `.cancel()` before the invocation starts:

```typescript
const run = await counter.increment.schedule(Datetime.fromDate(new Date(Date.now() + 60_000)));
// …later, if the run is no longer wanted:
const withdrawn = await run.cancel();
```

`cancel()` resolves to `true` when it withdrew the invocation, `false` when it already finished or was already cancelled, and rejects with a `cannot-cancel-running` `RemoteCallError` when it is running. Awaiting a cancelled invocation with `run.awaitResult()` rejects with `cancelled`.

To cancel from a later invocation of the same agent, keep `run.id.toString()` in agent state and rebuild the handle without a remote call:

```typescript
import { Invocation, InvocationId } from '@golemcloud/golem-ts-sdk';

await Invocation.get(InvocationId.parse(this.pendingRun), Counter.methods.increment).cancel();
```

The ID only locates the invocation; it is not authority. Only the agent that scheduled the invocation, or a caller with management permission, can observe or cancel it. `.schedule()` never sends a scope card and rejects methods with streams.

(To withdraw an awaited call, pass `{ signal }` as the trailing option, for example `method(input, { signal })`, and abort it: an abort before the invocation starts withdraws it on a best-effort basis, and an abort after it starts only stops waiting. You can also gate a method on agent state, or cancel from the CLI with a known idempotency key via `golem invocation cancel` — see `golem-cancel-queued-invocation`.)

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
