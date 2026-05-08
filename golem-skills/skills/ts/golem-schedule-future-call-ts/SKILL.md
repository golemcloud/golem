---
name: golem-schedule-future-call-ts
description: "Scheduling a future agent invocation in a TypeScript Golem project. Use when the user asks about delayed invocations, scheduling calls for later, or timed agent execution."
---

# Scheduling a Future Agent Invocation (TypeScript)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

## Usage

Every method on the generated client has a `.schedule()` variant that takes a `Datetime` as the first argument:

```typescript
import { Datetime } from 'golem:rpc/types@0.2.2';

const counter = CounterAgent.get("my-counter");

// Schedule increment to run 60 seconds from now
const nowSecs = BigInt(Math.floor(Date.now() / 1000));

counter.increment.schedule({
    seconds: nowSecs + 60n,
    nanoseconds: 0,
});

// Schedule with arguments
const reporter = ReportAgent.get("daily");
reporter.generateReport.schedule(
    { seconds: BigInt(tomorrowMidnight), nanoseconds: 0 },
    "summary",
);
```

## Datetime Type

The `Datetime` object represents a point in time as seconds + nanoseconds since the Unix epoch:

```typescript
import { Datetime } from 'golem:rpc/types@0.2.2';

const dt: Datetime = {
    seconds: BigInt(1700000000),  // Unix timestamp as BigInt
    nanoseconds: 0,               // Sub-second precision
};
```

Note: `seconds` is a `BigInt` in the TypeScript binding.

## Cancelable Variant

Every method also has a `.scheduleCancelable()` variant that returns a `CancellationToken`. Call `.cancel()` on the token to prevent the scheduled invocation from firing:

```typescript
import { CancellationToken } from '@golemcloud/golem-ts-sdk';

const token: CancellationToken = counter.increment.scheduleCancelable({
    seconds: nowSecs + 60n,
    nanoseconds: 0,
});

// Later, to cancel the pending invocation:
token.cancel();
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
