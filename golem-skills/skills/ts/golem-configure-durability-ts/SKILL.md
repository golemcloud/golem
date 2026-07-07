---
name: golem-configure-durability-ts
description: "Choosing between durable and ephemeral agents in a TypeScript Golem project. Use when the user asks about agent durability modes, making an agent stateless, or configuring agent persistence."
---

# Configuring Agent Durability (TypeScript)

## Durable Agents (Default)

By default, all Golem agents are **durable**:

- State persists across invocations, failures, and restarts
- Every side effect is recorded in an **oplog** (operation log)
- On failure, the agent is transparently recovered by replaying the oplog
- No special code needed — durability is automatic

> **You cannot opt out of oplog writes for a durable agent.** The oplog is how durability works — every side effect must be recorded. If you are worried about oplog volume or replay cost (long-running agents, heartbeats, polling, recurring tasks), do *not* try to skip persistence. Use **durable with periodic snapshots** instead (see below).

A standard durable agent — no `mode` is needed since durable is the default:

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const CounterAgent = defineAgent({
    name: 'CounterAgent',
    id: { name: z.string() },
    methods: {
        increment: method({ input: {}, returns: z.number() }),
        getCount: method({ input: {}, returns: z.number() }),
    },
});

export const CounterAgentImpl = CounterAgent.implement({
    init: () => ({ value: 0 }),
    methods: {
        increment() {
            this.value += 1;
            return this.value;
        },
        getCount() {
            return this.value;
        },
    },
});
```

## Durable with Periodic Snapshots

Same durability guarantees as the default durable mode, but recovery starts from the **latest snapshot** instead of replaying the full oplog from the beginning. Use this whenever the oplog grows unboundedly — long-running agents, high-frequency state changes, **heartbeats, polling loops, recurring tasks**. Add a `snapshotting` option to `defineAgent(...)`:

```typescript
// snapshot every 10 successful invocations
defineAgent({
    name: 'CounterAgent',
    id: { name: z.string() },
    snapshotting: { state: z.object({ value: z.number() }), policy: { everyNInvocations: 10 } },
    methods: { /* ... */ },
});

// or at most once per 30-second interval
defineAgent({
    name: 'HeartbeatAgent',
    id: { name: z.string() },
    snapshotting: { state: z.object({ /* ... */ }), policy: { periodicSeconds: 30 } },
    methods: { /* ... */ },
});
```

See [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md) for snapshotting policies, typed `state` schemas, and custom `save` / `load`.

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is not needed:

- State is discarded after each invocation completes
- The oplog is not replayed — lower overhead
- Useful for pure functions, request handlers, or adapters

Set `mode: 'ephemeral'` on the spec:

```typescript
export const StatelessHandler = defineAgent({
    name: 'StatelessHandler',
    mode: 'ephemeral',
    id: { name: z.string() },
    methods: { handle: method({ input: { input: z.string() }, returns: z.string() }) },
});

StatelessHandler.implement({
    init: () => ({}),
    methods: {
        handle({ input }) {
            return `processed: ${input}`;
        },
    },
});
```

## When to Choose Which

| Use Case | Mode |
|----------|------|
| Counter, shopping cart, workflow orchestrator | **Durable** (default) |
| Stateless request processor, transformer | **Ephemeral** |
| Long-running saga or multi-step pipeline | **Durable** (default) |
| Pure computation, no side effects worth persisting | **Ephemeral** |
| Agent that calls external APIs with at-least-once semantics | **Durable** (default) |
| Long-running agent with heartbeats, polling, or recurring tasks | **Durable + periodic snapshots** |
| Any durable agent whose oplog grows so large that replay is slow | **Durable + periodic snapshots** |

When in doubt, use the default (durable). Ephemeral mode is an optimization for agents that genuinely don't need persistence. Add periodic snapshots whenever recovery time matters — see [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md).
