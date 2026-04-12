---
name: golem-atomic-block-rust
description: "Using atomic blocks, persistence control, idempotency, and oplog management in a Rust Golem project. Use when the user asks about atomically, persistence levels, idempotence mode, oplog commit, or idempotency keys."
---

# Atomic Blocks and Durability Controls (Rust)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

## Atomic Operations

Group side effects so they are retried together on failure:

```rust
use golem_rust::atomically;

let (a, b) = atomically(|| {
    let a = side_effect_1();
    let b = side_effect_2(a);
    (a, b)
});
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

## Persistence Level Control

Temporarily disable oplog recording for performance-sensitive sections:

```rust
use golem_rust::{with_persistence_level, PersistenceLevel};

with_persistence_level(PersistenceLevel::PersistNothing, || {
    // No oplog entries — side effects will be replayed on recovery
    // Use for idempotent operations where replay is safe
});
```

## Idempotence Mode

Control whether HTTP requests are retried when the result is uncertain:

```rust
use golem_rust::with_idempotence_mode;

with_idempotence_mode(false, || {
    // HTTP requests won't be automatically retried
    // Use for non-idempotent external API calls (e.g., payments)
});
```

## Oplog Commit

Wait until the oplog is replicated to a specified number of replicas before continuing:

```rust
use golem_rust::oplog_commit;

// Ensure oplog is replicated to 3 replicas before proceeding
oplog_commit(3);
```

## Idempotency Key Generation

Generate a durable idempotency key that persists across agent restarts — safe for payment APIs and other exactly-once operations:

```rust
use golem_rust::generate_idempotency_key;

let key = generate_idempotency_key();
// Use this key with external APIs to ensure exactly-once processing
```

## Retry Policy

Override the default retry policy for a block of code:

```rust
use golem_rust::{with_retry_policy, RetryPolicy};

with_retry_policy(RetryPolicy { /* ... */ }, || {
    // Code with custom retry behavior
});
```
