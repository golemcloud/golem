---
name: golem-atomic-block-scala
description: "Using atomic blocks, idempotency, checkpoints, and oplog management in a Scala Golem project. Use when the user asks about atomically, idempotence mode, oplog commit, checkpoints, or idempotency keys."
---

# Atomic Blocks and Durability Controls (Scala)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around idempotency or atomicity.

All guard and checkpoint APIs are `Future`-based — blocks must return `Future[A]`.

## Atomic Operations

Group **external, observable side effects** (HTTP calls, calls to other agents, file/network I/O) so that on a crash the whole group is replayed together. If the agent fails partway through the block, recovery will re-execute the **entire** block from the start instead of resuming from the middle — so any external effects performed before the crash will be performed again.

> **What this is NOT.** `atomically` is **not** an STM/transaction primitive and **not** for grouping in-memory state mutations. Golem agents are single-threaded, and in-memory state is automatically rebuilt by oplog replay on recovery, so wrapping plain in-memory updates in `atomically` does nothing useful. The terminology overlaps with Haskell STM, Scala STM, database transactions, and `synchronized` blocks, but the semantics are different: this is purely about how durable, externally-observable effects are re-executed across a crash boundary.
>
> **It is also NOT how you reduce oplog size or speed up recovery.** `atomically`, idempotency-mode, and checkpoint APIs do not shrink the oplog or skip replay. If your concern is that the oplog is growing too large or recovery/replay is becoming slow (long-running agents, heartbeats, polling, recurring tasks), use **snapshot-based recovery** instead — see [`golem-custom-snapshot-scala`](../golem-custom-snapshot-scala/SKILL.md). You cannot opt out of oplog writes for a durable agent.
>
> Use it only when you have **two or more external side effects** that must not be left in a "first one happened, second one didn't" state across a recovery.

Good use case — two external calls that must replay together:

```scala
import golem.Guards
import scala.concurrent.Future

// Reserve inventory and charge the customer — if we crash between them,
// we want recovery to re-run BOTH calls, not skip the reservation.
val order: Future[Order] = Guards.atomically {
  for {
    reservation <- inventoryApi.reserve(itemId, qty)
    charge      <- paymentApi.charge(customer, price)
  } yield Order(reservation, charge)
}
```

Bad use case — pure in-memory updates that already replay deterministically:

```scala
// DON'T do this. Wrapping in-memory mutations adds nothing — the oplog
// already rebuilds `state.balance` and `state.lastTx` deterministically.
Guards.atomically {
  state.balance -= amount
  state.lastTx = now
  Future.successful(())
}
```

## Custom Durability for Libraries

The low-level `golem:durability@1.6.0` interface is intended for SDK and library authors, not as an application tuning knob. It lets a library represent several raw host effects as one custom durable invocation. `DurabilityApi.durable` and `DurabilityApi.durableAsync` own the live invocation resource: they evaluate the body only on the live path, finish and persist its typed result, or return the recorded result on replay.

A returned `Left` is a normal typed result: the combinator finishes the invocation, persists the error, and returns the same error on replay. A thrown exception, failed `Future`, trap, or cancellation before `finish` instead drops the unfinished resource without recording an `End`, so recovery retries the whole custom operation. Library authors must make repeated attempts safe with the correct operation classification, external idempotency keys, or transactions.

## Idempotence Mode

> **Default: `true`.** Every outgoing HTTP request — including `POST`, `PUT`, `PATCH`, and
> `DELETE` — is treated as idempotent. This means status-code-keyed retry policies (see
> `golem-retry-policies-scala`) **already work out of the box for `POST` requests**. You do
> **not** need to wrap a `POST` in `Guards.withIdempotenceMode(true) { ... }` to make it
> retriable on a 5xx — that is the default.

Use `Guards.withIdempotenceMode(false) { ... }` only when you need to **opt out** for a specific
call. The flag controls how `WriteRemote` host functions are replayed when their previous
attempt's outcome is unknown after a crash:

- `true` (default): assume the previous attempt succeeded; do **not** re-invoke on replay.
  Combined with the host-side retry machinery, the request can be transparently re-sent when a
  matching retry policy fires.
- `false`: do **not** assume success; the worker traps so a higher-level retry decides what to
  do. Use this for non-idempotent side effects whose accidental duplication would be more harmful
  than missing the call entirely.

```scala
import golem.Guards
import scala.concurrent.Future

// Opt OUT of the default — the wrapped call is treated as non-idempotent.
val result: Future[Unit] = Guards.withIdempotenceMode(false) {
  // HTTP requests will not be automatically retried on uncertain outcomes
  Future.successful(())
}
```

## Oplog Commit

Wait until the oplog is replicated to a specified number of replicas before continuing:

```scala
import golem.HostApi

// Ensure oplog is replicated to 3 replicas before proceeding
HostApi.oplogCommit(3)
```

## Idempotency Key Generation

Generate a durable idempotency key that persists across agent restarts — safe for payment APIs and other exactly-once operations:

```scala
import golem.HostApi

val key = HostApi.generateIdempotencyKey()
// Use this key with external APIs to ensure exactly-once processing
```

## Retry Policy

Override the default retry policy for a block of code:

```scala
import golem.Guards
import scala.concurrent.Future

Guards.withRetryPolicy(policy) {
  // Code with custom retry behavior
  Future.successful(())
}
```

## Checkpoints

Capture an oplog position and revert execution to it on failure:

```scala
import golem.Checkpoint
import scala.concurrent.Future

// Create a checkpoint and use it manually
val cp = Checkpoint()
cp.assertOrRevert(condition)       // revert if false

// tryOrRevert — revert if the Future fails
val result: Future[Int] = cp.tryOrRevert {
  Future.successful(42)
}

// runOrRevert — revert if the Future resolves to a Left
val result: Future[Int] = cp.runOrRevert {
  Future.successful(Right(42))
}
```

### Scoped checkpoints

```scala
import golem.Checkpoint
import scala.concurrent.Future

// withCheckpointTry — revert if the Future fails
val result: Future[Int] = Checkpoint.withCheckpointTry { cp =>
  cp.assertOrRevert(someCondition)
  Future.successful(42)
}

// withCheckpoint — revert if the Future resolves to a Left
val result: Future[Int] = Checkpoint.withCheckpoint { cp =>
  Future.successful(Right(42))
}
```
