---
name: golem-recurring-task-ts
description: "Implementing a recurring (cron-like) task in a TypeScript Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (TypeScript)

## Overview

A Golem agent can act as its own scheduler by scheduling one of its own methods to run again at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

Because a method handler's `this` is bound to the agent's **state** (not to its other methods), factor the self-scheduling logic into a small module-level helper that uses the definition's RPC client and calls `.schedule()`.

## Basic Pattern

The agent schedules its own `poll` method to run again after a delay.
Call `start` only once per agent identity in the basic and backoff variants;
each call enqueues another independent recurring chain:

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const PollerAgent = defineAgent({
    name: 'PollerAgent',
    id: { name: z.string() },
    methods: {
        start: method({ input: {}, returns: z.void() }),
        poll: method({ input: {}, returns: z.void() }),
    },
});

// Self-scheduling helper: enqueue this agent's own `poll` to run after a delay.
function scheduleNext(name: string, delaySecs: bigint): void {
    const nowSecs = BigInt(Math.floor(Date.now() / 1000));
    PollerAgent.client.get({ name }).poll.schedule({ seconds: nowSecs + delaySecs, nanoseconds: 0 });
}

export const PollerAgentImpl = PollerAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        start() {
            // Kick off the loop by enqueueing the first poll.
            scheduleNext(this.name, 0n);
        },
        poll() {
            // 1. Do the recurring work.
            doWork();
            // 2. Schedule the next run (60 seconds from now).
            scheduleNext(this.name, 60n);
        },
    },
});
```

## Exponential Backoff

Increase the delay on repeated failures, reset on success. Keep the counters in the agent's state:

```typescript
export const PollerAgentImpl = PollerAgent.implement({
    init: ({ id }) => ({
        name: id.name,
        consecutiveFailures: 0,
        baseIntervalSecs: 60n,
        maxIntervalSecs: 3600n,
    }),
    methods: {
        start() {
            scheduleNext(this.name, 0n);
        },
        poll() {
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

            scheduleNext(this.name, delay);
        },
    },
});
```

## Cancellation

For a restartable loop, include a generation in every scheduled tick. `cancel`
stops work logically; a pending tick still arrives but exits without doing work.
Repeated `start` calls while running are no-ops. After cancellation, `start`
advances the generation so an old tick cannot create a second chain.

This complete variant replaces the basic definition and implementation. It stores
only logical state, which the typed snapshot schema can restore:

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

const PollerState = z.object({
    name: z.string(), running: z.boolean(), generation: z.number(),
});

export const PollerAgent = defineAgent({
    name: 'PollerAgent',
    id: { name: z.string() },
    snapshotting: { state: PollerState, policy: { everyNInvocations: 10 } },
    methods: {
        start: method({ input: {}, returns: z.void() }),
        poll: method({ input: { generation: z.number() }, returns: z.void() }),
        cancel: method({ input: {}, returns: z.void() }),
    },
});

function scheduleNext(name: string, generation: number, delaySecs: bigint): void {
    const nowSecs = BigInt(Math.floor(Date.now() / 1000));
    PollerAgent.client.get({ name }).poll.schedule({
        seconds: nowSecs + delaySecs, nanoseconds: 0,
    }, { generation });
}

export const PollerAgentImpl = PollerAgent.implement<z.infer<typeof PollerState>>({
    init: ({ id }) => ({
        name: id.name,
        running: false,
        generation: 0,
    }),
    methods: {
        start() {
            if (this.running) return;
            this.running = true;
            this.generation++;
            scheduleNext(this.name, this.generation, 0n);
        },
        poll({ generation }) {
            if (!this.running || this.generation !== generation) return;
            console.log('Polling', this.name); // Replace with recurring work.
            scheduleNext(this.name, generation, 60n);
        },
        cancel() {
            this.running = false;
            this.generation++;
        },
    },
});
```

Restore the running flag and generation together. Do not reset the generation or
enqueue a replacement during restoration: an existing scheduled tick may still arrive.

For best-effort removal before a pending invocation starts, `.schedule()` returns
a `CancellationToken` with `.cancel()`. That live host resource is not snapshot
state. Keep the generation check even when using a token; cancellation cannot
stop work that has already started. The logical variant above needs no token or
custom snapshot hooks.

### Cancellation from the CLI

For the basic no-input `poll` contract, schedule from the CLI with an explicit
idempotency key, then cancel the pending invocation by key:

```shell
# Schedule with a known idempotency key
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' 'PollerAgent("my-poller")' poll

# Cancel the pending invocation
golem agent cancel-invocation 'PollerAgent("my-poller")' 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```typescript
poll() {
    const items = fetchPendingItems();
    for (const item of items) {
        process(item);
    }
    scheduleNext(this.name, 60n);
}
```

### Periodic Cleanup

Replace the basic pattern's `poll` handler with cleanup work (and initialize
`entries` in state):

```typescript
poll() {
    this.entries = this.entries.filter((e) => !e.isExpired());
    scheduleNext(this.name, 3600n); // run hourly
}
```

### Heartbeat / Keep-Alive

Replace the basic pattern's `poll` handler with heartbeat work (and initialize
`serviceUrl` in state):

```typescript
poll() {
    sendHeartbeat(this.serviceUrl);
    scheduleNext(this.name, 30n); // every 30s
}
```

## Helper for Scheduling Self

Keep the scheduling logic in one module-level helper so every method stays clean. `PollerAgent.client` is the typed RPC factory for this same agent type; addressing it by the agent's own id record targets this instance:

```typescript
function scheduleNext(name: string, delaySecs: bigint): void {
    const nowSecs = BigInt(Math.floor(Date.now() / 1000));
    PollerAgent.client.get({ name }).poll.schedule({ seconds: nowSecs + delaySecs, nanoseconds: 0 });
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `.schedule()` call is a fire-and-forget enqueue; the current invocation completes immediately
- Use a state flag or generation counter to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery

## Recovery & Oplog Growth

Each scheduled tick (heartbeat, poll, cleanup) appends entries to the agent's oplog. For long-running or high-frequency recurring tasks, the oplog grows unboundedly, and recovery on crash will replay the full history — which becomes slow over time.

**You cannot opt out of oplog writes for a durable agent.** Enable snapshot-based recovery with a schema for every ordinary state field and a policy, as in the cancellation variant above. For the basic pattern, use `snapshotting: { state: z.object({ name: z.string() }), policy: { everyNInvocations: 10 } }`. A policy without a state schema requires custom `snapshot: { save, load }` hooks, even for JSON state. See [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md).
