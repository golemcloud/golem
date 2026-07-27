---
name: golem-atomic-block-rust
description: "Using atomic blocks, persistence control, idempotency, and oplog management in a Rust Golem project. Use when the user asks about atomically, persistence levels, idempotence mode, oplog commit, or idempotency keys."
---

# Atomic Blocks and Durability Controls (Rust)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

## Atomic Operations

Group **external, observable side effects** (HTTP calls, calls to other agents, file/network I/O) so that on a crash the whole group is replayed together. If the agent fails partway through the block, recovery will re-execute the **entire** block from the start instead of resuming from the middle — so any external effects performed before the crash will be performed again.

> **What this is NOT.** `atomically` is **not** an STM/transaction primitive and **not** for grouping in-memory state mutations. Golem agents are single-threaded, and in-memory state is automatically rebuilt by oplog replay on recovery, so wrapping plain in-memory updates in `atomically` does nothing useful. The terminology overlaps with Haskell STM, database transactions, and `synchronized` blocks, but the semantics are different: this is purely about how durable, externally-observable effects are re-executed across a crash boundary.
>
> **It is also NOT how you reduce oplog size or speed up recovery.** Despite the description's mention of "oplog management" and "persistence control", `atomically`/persistence-level/idempotency-mode APIs do not shrink the oplog or skip replay. If your concern is that the oplog is growing too large or recovery/replay is becoming slow (long-running agents, heartbeats, polling, recurring tasks), use **snapshot-based recovery** instead — see [`golem-custom-snapshot-rust`](../golem-custom-snapshot-rust/SKILL.md). You cannot opt out of oplog writes for a durable agent.
>
> Use it only when you have **two or more external side effects** that must not be left in a "first one happened, second one didn't" state across a recovery.

Good use case — two external calls that must replay together:

```rust
use golem_rust::atomically;

// Reserve inventory and charge the customer — if we crash between them,
// we want recovery to re-run BOTH calls, not skip the reservation.
let order_id = atomically(|| {
    let reservation = inventory_api::reserve(item_id, qty);
    let charge = payment_api::charge(customer, price);
    (reservation, charge)
});
```

### Async version

```rust
use golem_rust::atomically_async;

let order_id = atomically_async(|| async {
    let reservation = inventory_api::reserve(item_id, qty).await;
    let charge = payment_api::charge(customer, price).await;
    (reservation, charge)
}).await;
```

Bad use case — pure in-memory updates that already replay deterministically:

```rust
// DON'T do this. Wrapping in-memory mutations adds nothing — the oplog
// already rebuilds `self.balance` and `self.last_tx` deterministically.
atomically(|| {
    self.balance -= amount;
    self.last_tx = now;
});
```

## Persistence Level Control

Adjust how the oplog is interpreted for a section of code. Setting the level to `PersistNothing` does **not** disable oplog recording — entries are still written, but they are treated only as an observable log and are **not used for replay**. On recovery, the side effects are **not** re-executed and **not** replayed; if the block naively runs the same side effects during replay, recovery will fail.

This is **not** a knob for application code. Its primary use case is **authoring Golem-specific libraries** that implement their own custom durability on top of raw side effects. Code inside such a block must:

1. Explicitly check whether the agent is in live or replay mode (via the durability API).
2. Skip the raw side effects during replay.
3. Use the durability APIs to record/recover state in a custom way.

```rust
use golem_rust::{with_persistence_level, PersistenceLevel};

with_persistence_level(PersistenceLevel::PersistNothing, || {
    // Oplog entries here are observable only, never used for replay.
    // The block MUST check live vs replay mode and use custom durability
    // primitives — naively running side effects will break recovery.
});
```

### Async version

```rust
use golem_rust::{with_persistence_level_async, PersistenceLevel};

with_persistence_level_async(PersistenceLevel::PersistNothing, || async {
    // Same constraints as the sync version — custom durability required.
}).await;
```

## Idempotence Mode

> **Default: `true`.** Every outgoing HTTP request — including `POST`, `PUT`, `PATCH`, and
> `DELETE` — is treated as idempotent. This means status-code-keyed retry policies (see
> `golem-retry-policies-rust`) **already work out of the box for `POST` requests**. You do **not**
> need to wrap a `POST` in `with_idempotence_mode(true, ...)` to make it retriable on a 5xx — that
> is the default.

Use `with_idempotence_mode(false, ...)` only when you need to **opt out** for a specific call. The
flag controls how `WriteRemote` host functions are replayed when their previous attempt's outcome
is unknown after a crash:

- `true` (default): assume the previous attempt succeeded; do **not** re-invoke on replay.
  Combined with the host-side retry machinery, the request can be transparently re-sent when a
  matching retry policy fires.
- `false`: do **not** assume success; the worker traps so a higher-level retry decides what to
  do. Use this for non-idempotent side effects whose accidental duplication would be more harmful
  than missing the call entirely.

```rust
use golem_rust::with_idempotence_mode;

// Opt OUT of the default — the wrapped call is treated as non-idempotent.
with_idempotence_mode(false, || {
    // HTTP requests will not be automatically retried on uncertain outcomes
});
```

### Async version

```rust
use golem_rust::with_idempotence_mode_async;

with_idempotence_mode_async(false, || async {
    // HTTP requests will not be automatically retried on uncertain outcomes
}).await;
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

### Async version

```rust
use golem_rust::{with_retry_policy_async, RetryPolicy};

with_retry_policy_async(RetryPolicy { /* ... */ }, || async {
    // Code with custom retry behavior
}).await;
```
