---
name: golem-atomic-block-ts
description: "Using atomic blocks, idempotency, and oplog management in a TypeScript Golem project. Use when the user asks about atomically, idempotence mode, oplog commit, or idempotency keys."
---

# Atomic Blocks and Durability Controls (TypeScript)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around idempotency or atomicity.

The scoped helpers (`atomically`, `withIdempotenceMode`, `withRetryPolicy`) accept both sync and async callbacks. When an async callback is passed, the function returns a `Promise`.

## Atomic Operations

Group **external, observable side effects** (HTTP calls, calls to other agents, file/network I/O) so that on a crash the whole group is replayed together. If the agent fails partway through the block, recovery will re-execute the **entire** block from the start instead of resuming from the middle — so any external effects performed before the crash will be performed again.

This also applies to a **single** external call when its result is followed by a thrown exception. For example, `fetch()` returning HTTP `500` is still a successfully completed HTTP side effect; the later `throw new Error(...)` based on `response.ok` is a separate failure after the response has already been recorded. Without `atomically`, retry will replay the recorded `500` response instead of sending a new request. Wrap the fetch, response-body read, and status check in the same atomic block when a status-based failure should retry the HTTP call itself.

> **What this is NOT.** `atomically` is **not** an STM/transaction primitive and **not** for grouping in-memory state mutations. Golem agents are single-threaded, and in-memory state is automatically rebuilt by oplog replay on recovery, so wrapping plain in-memory updates in `atomically` does nothing useful. The terminology overlaps with Haskell STM, database transactions, and `synchronized` blocks, but the semantics are different: this is purely about how durable, externally-observable effects are re-executed across a crash boundary.
>
> **It is also NOT how you reduce oplog size or speed up recovery.** `atomically` and idempotency-mode APIs do not shrink the oplog or skip replay. If your concern is that the oplog is growing too large or recovery/replay is becoming slow (long-running agents, heartbeats, polling, recurring tasks), use **snapshot-based recovery** instead — see [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md). You cannot opt out of oplog writes for a durable agent.
>
> Use it only when you have **two or more external side effects** that must not be left in a "first one happened, second one didn't" state across a recovery, or when a single external side effect is immediately followed by validation that may throw and must cause that side effect to be retried.

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

Good use case — one HTTP call whose non-2xx result must retry the call, not replay the failed
response:

```typescript
import { atomically } from '@golemcloud/golem-ts-sdk';

const payment = await atomically(async () => {
    const response = await fetch('https://payments.example.com/charge', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ orderId, amount }),
    });

    if (!response.ok) {
        const body = await response.text();
        throw new Error(`payment failed: ${response.status} ${body}`);
    }

    return await response.json();
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

## Custom Durability for Libraries

The low-level `golem:durability@1.6.0` interface is intended for SDK and library authors, not as an application tuning knob. It lets a library represent several raw host effects as one custom durable invocation. The SDK's `durable(spec, request, body)` combinator owns the live invocation resource: it evaluates a synchronous or asynchronous body only on the live path, finishes and persists its typed result, or returns the recorded result on replay.

A returned `Result.err(...)` is a normal typed result: the combinator finishes the invocation, persists the error, and returns the same error on replay. A throw, rejected promise, trap, or cancellation before `finish` instead drops the unfinished resource without recording an `End`, so recovery retries the whole custom operation. Library authors must make repeated attempts safe with the correct operation classification, external idempotency keys, or transactions.

## Idempotence Mode

> **Default: `true`.** Every outgoing HTTP request — including `POST`, `PUT`, `PATCH`, and
> `DELETE` — is treated as idempotent. This means status-code-keyed retry policies (see
> `golem-retry-policies-ts`) **already work out of the box for `POST` requests**. You do **not**
> need to wrap a `POST` in `withIdempotenceMode(true, ...)` to make it retriable on a 5xx — that
> is the default.

Use `withIdempotenceMode(false, ...)` only when you need to **opt out** for a specific call. The
flag controls how `WriteRemote` host functions are replayed when their previous attempt's outcome
is unknown after a crash:

- `true` (default): assume the previous attempt succeeded; do **not** re-invoke on replay.
  Combined with the host-side retry machinery, the request can be transparently re-sent when a
  matching retry policy fires.
- `false`: do **not** assume success; the worker traps so a higher-level retry decides what to
  do. Use this for non-idempotent side effects whose accidental duplication would be more harmful
  than missing the call entirely.

```typescript
import { withIdempotenceMode } from '@golemcloud/golem-ts-sdk';

// Opt OUT of the default — the wrapped call is treated as non-idempotent.
// Sync
withIdempotenceMode(false, () => {
    // HTTP requests will not be automatically retried on uncertain outcomes
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
