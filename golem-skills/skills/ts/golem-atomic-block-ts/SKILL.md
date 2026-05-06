---
name: golem-atomic-block-ts
description: "Using atomic blocks, persistence control, idempotency, and oplog management in a TypeScript Golem project. Use when the user asks about atomically, persistence levels, idempotence mode, oplog commit, or idempotency keys."
---

# Atomic Blocks and Durability Controls (TypeScript)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

All helper functions (`atomically`, `withPersistenceLevel`, `withIdempotenceMode`, `withRetryPolicy`) accept both sync and async callbacks. When an async callback is passed, the function returns a `Promise`.

## Atomic Operations

Group **external, observable side effects** (HTTP calls, calls to other agents, file/network I/O) so that on a crash the whole group is replayed together. If the agent fails partway through the block, recovery will re-execute the **entire** block from the start instead of resuming from the middle — so any external effects performed before the crash will be performed again.

> **What this is NOT.** `atomically` is **not** an STM/transaction primitive and **not** for grouping in-memory state mutations. Golem agents are single-threaded, and in-memory state is automatically rebuilt by oplog replay on recovery, so wrapping plain in-memory updates in `atomically` does nothing useful. The terminology overlaps with Haskell STM, database transactions, and `synchronized` blocks, but the semantics are different: this is purely about how durable, externally-observable effects are re-executed across a crash boundary.
>
> **It is also NOT how you reduce oplog size or speed up recovery.** Despite the description's mention of "oplog management" and "persistence control", `atomically`/persistence-level/idempotency-mode APIs do not shrink the oplog or skip replay. If your concern is that the oplog is growing too large or recovery/replay is becoming slow (long-running agents, heartbeats, polling, recurring tasks), use **snapshot-based recovery** instead — see [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md). You cannot opt out of oplog writes for a durable agent.
>
> Use it only when you have **two or more external side effects** that must not be left in a "first one happened, second one didn't" state across a recovery.

Good use case — two external calls that must replay together:

```typescript
import { atomically } from '@golemcloud/golem-ts-sdk';

// Reserve inventory and charge the customer — if we crash between them,
// we want recovery to re-run BOTH calls, not skip the reservation.

// Sync
const order = atomically(() => {
    const reservation = inventoryApi.reserve(itemId, qty);
    const charge = paymentApi.charge(customer, price);
    return { reservation, charge };
});

// Async
const order = await atomically(async () => {
    const reservation = await inventoryApi.reserve(itemId, qty);
    const charge = await paymentApi.charge(customer, price);
    return { reservation, charge };
});
```

Bad use case — pure in-memory updates that already replay deterministically:

```typescript
// DON'T do this. Wrapping in-memory mutations adds nothing — the oplog
// already rebuilds `this.balance` and `this.lastTx` deterministically.
atomically(() => {
    this.balance -= amount;
    this.lastTx = now;
});
```

## Persistence Level Control

Adjust how the oplog is interpreted for a section of code. Setting the level to `persist-nothing` does **not** disable oplog recording — entries are still written, but they are treated only as an observable log and are **not used for replay**. On recovery, the side effects are **not** re-executed and **not** replayed; if the block naively runs the same side effects during replay, recovery will fail.

This is **not** a knob for application code. Its primary use case is **authoring Golem-specific libraries** that implement their own custom durability on top of raw side effects. Code inside such a block must:

1. Explicitly check whether the agent is in live or replay mode (via the durability API).
2. Skip the raw side effects during replay.
3. Use the durability APIs to record/recover state in a custom way.

```typescript
import { withPersistenceLevel } from '@golemcloud/golem-ts-sdk';

// Sync
withPersistenceLevel({ tag: 'persist-nothing' }, () => {
    // Oplog entries here are observable only, never used for replay.
    // The block MUST check live vs replay mode and use custom durability
    // primitives — naively running side effects will break recovery.
});

// Async
await withPersistenceLevel({ tag: 'persist-nothing' }, async () => {
    // Same constraints as the sync version — custom durability required.
});
```

## Idempotence Mode

Control whether HTTP requests are retried when the result is uncertain:

```typescript
import { withIdempotenceMode } from '@golemcloud/golem-ts-sdk';

// Sync
withIdempotenceMode(false, () => {
    // HTTP requests won't be automatically retried
});

// Async
await withIdempotenceMode(false, async () => {
    await nonIdempotentApiCall();
});
```

## Oplog Commit

Wait until the oplog is replicated to a specified number of replicas before continuing:

```typescript
import { oplogCommit } from '@golemcloud/golem-ts-sdk';

// Ensure oplog is replicated to 3 replicas before proceeding
oplogCommit(3);
```

## Idempotency Key Generation

Generate a durable idempotency key that persists across agent restarts — safe for payment APIs and other exactly-once operations:

```typescript
import { generateIdempotencyKey } from '@golemcloud/golem-ts-sdk';

const key = generateIdempotencyKey();
// Use this key with external APIs to ensure exactly-once processing
```

## Retry Policy

Override the default retry policy for a block of code:

```typescript
import { withRetryPolicy } from '@golemcloud/golem-ts-sdk';

// Sync
withRetryPolicy({ /* ... */ }, () => {
    // Code with custom retry behavior
});

// Async
await withRetryPolicy({ /* ... */ }, async () => {
    await someRetryableOperation();
});
```
