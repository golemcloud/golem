---
name: golem-atomic-block-ts
description: "Using atomic blocks, persistence control, idempotency, and oplog management in a TypeScript Golem project. Use when the user asks about atomically, persistence levels, idempotence mode, oplog commit, or idempotency keys."
---

# Atomic Blocks and Durability Controls (TypeScript)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

All helper functions (`atomically`, `withPersistenceLevel`, `withIdempotenceMode`, `withRetryPolicy`) accept both sync and async callbacks. When an async callback is passed, the function returns a `Promise`.

## Atomic Operations

Group side effects so they are retried together on failure:

```typescript
import { atomically } from '@golemcloud/golem-ts-sdk';

// Sync
const [a, b] = atomically(() => {
    const a = sideEffect1();
    const b = sideEffect2(a);
    return [a, b];
});

// Async
const [a, b] = await atomically(async () => {
    const a = await sideEffect1();
    const b = await sideEffect2(a);
    return [a, b];
});
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

## Persistence Level Control

Temporarily disable oplog recording for performance-sensitive sections:

```typescript
import { withPersistenceLevel } from '@golemcloud/golem-ts-sdk';

// Sync
withPersistenceLevel({ tag: 'persist-nothing' }, () => {
    // No oplog entries — side effects will be replayed on recovery
});

// Async
await withPersistenceLevel({ tag: 'persist-nothing' }, async () => {
    await someAsyncOperation();
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
