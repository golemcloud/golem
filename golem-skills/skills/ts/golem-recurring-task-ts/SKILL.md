---
name: golem-recurring-task-ts
description: "Implementing a recurring (cron-like) task in a TypeScript Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (TypeScript)

## Overview

A Golem agent can act as its own scheduler by calling `.schedule()` on itself at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

The agent schedules its own method to run again after a delay:

```typescript
import { agent, BaseAgent } from '@golemcloud/golem-ts-sdk';

@agent()
class PollerAgent extends BaseAgent {
    name: string;

    constructor(name: string) {
        super();
        this.name = name;
    }

    start(): void {
        this.poll();
    }

    poll(): void {
        // 1. Do the recurring work
        doWork();

        // 2. Schedule the next run (60 seconds from now)
        const self = PollerAgent.get(this.name);
        const nowSecs = BigInt(Math.floor(Date.now() / 1000));
        self.poll.schedule({ seconds: nowSecs + 60n, nanoseconds: 0 });
    }
}
```

## Exponential Backoff

Increase the delay on repeated failures, reset on success:

```typescript
@agent()
class PollerAgent extends BaseAgent {
    name: string;
    consecutiveFailures: number = 0;
    baseIntervalSecs: bigint = 60n;
    maxIntervalSecs: bigint = 3600n;

    constructor(name: string) {
        super();
        this.name = name;
    }

    poll(): void {
        const success = tryWork();

        let delay: bigint;
        if (success) {
            this.consecutiveFailures = 0;
            delay = this.baseIntervalSecs;
        } else {
            this.consecutiveFailures++;
            const exp = Math.min(this.consecutiveFailures, 6);
            delay = this.baseIntervalSecs * BigInt(2 ** exp);
            if (delay > this.maxIntervalSecs) delay = this.maxIntervalSecs;
        }

        const self = PollerAgent.get(this.name);
        const nowSecs = BigInt(Math.floor(Date.now() / 1000));
        self.poll.schedule({ seconds: nowSecs + delay, nanoseconds: 0 });
    }
}
```

## Cancellation with CancellationToken

Every method on the generated client has a `.scheduleCancelable()` variant that returns a `CancellationToken`. Store the token and call `.cancel()` to prevent the scheduled invocation from firing:

```typescript
import { CancellationToken } from '@golemcloud/golem-ts-sdk';

@agent()
class PollerAgent extends BaseAgent {
    name: string;
    cancelled: boolean = false;
    pendingToken: CancellationToken | undefined;

    constructor(name: string) {
        super();
        this.name = name;
    }

    poll(): void {
        if (this.cancelled) {
            return;
        }

        doWork();

        const self = PollerAgent.get(this.name);
        const nowSecs = BigInt(Math.floor(Date.now() / 1000));
        this.pendingToken = self.poll.scheduleCancelable(
            { seconds: nowSecs + 60n, nanoseconds: 0 },
        );
    }

    cancel(): void {
        this.cancelled = true;
        if (this.pendingToken) {
            this.pendingToken.cancel();
            this.pendingToken = undefined;
        }
    }
}
```

### Cancellation via State Flag

For simpler cases, just use a boolean flag — the next scheduled `poll` checks it and exits early:

```typescript
poll(): void {
    if (this.cancelled) return;
    doWork();
    this.scheduleNext(60n);
}

cancel(): void {
    this.cancelled = true;
}
```

### Cancellation from the CLI

Schedule with an explicit idempotency key and cancel the pending invocation:

```shell
# Schedule with a known idempotency key
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' 'PollerAgent("my-poller")' poll

# Cancel the pending invocation
golem agent invocation cancel 'PollerAgent("my-poller")' 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```typescript
poll(): void {
    const items = fetchPendingItems();
    for (const item of items) {
        process(item);
    }
    this.scheduleNext(60n);
}
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```typescript
cleanup(): void {
    this.entries = this.entries.filter(e => !e.isExpired());
    this.scheduleNext(3600n); // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```typescript
heartbeat(): void {
    sendHeartbeat(this.serviceUrl);
    this.scheduleNext(30n); // every 30s
}
```

## Helper for Scheduling Self

Extract the scheduling logic into a helper to keep methods clean:

```typescript
private scheduleNext(delaySecs: bigint): void {
    const self = PollerAgent.get(this.name);
    const nowSecs = BigInt(Math.floor(Date.now() / 1000));
    self.poll.schedule({ seconds: nowSecs + delaySecs, nanoseconds: 0 });
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `.schedule()` call is a fire-and-forget enqueue; the current invocation completes immediately
- Use a state flag or generation counter to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery
