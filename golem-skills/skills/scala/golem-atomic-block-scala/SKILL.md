---
name: golem-atomic-block-scala
description: "Using atomic blocks, persistence control, idempotency, checkpoints, and oplog management in a Scala Golem project. Use when the user asks about atomically, persistence levels, idempotence mode, oplog commit, checkpoints, or idempotency keys."
---

# Atomic Blocks and Durability Controls (Scala)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

All guard and checkpoint APIs are `Future`-based — blocks must return `Future[A]`.

## Atomic Operations

Group side effects so they are retried together on failure:

```scala
import golem.Guards
import scala.concurrent.Future

val result: Future[String] = Guards.atomically {
  sideEffect1()
  sideEffect2()
  Future.successful("done")
}
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

## Persistence Level Control

Temporarily disable oplog recording for performance-sensitive sections:

```scala
import golem.{Guards, HostApi}
import scala.concurrent.Future

val result: Future[Unit] = Guards.withPersistenceLevel(HostApi.PersistenceLevel.PersistNothing) {
  // No oplog entries — side effects will be replayed on recovery
  Future.successful(())
}
```

## Idempotence Mode

Control whether HTTP requests are retried when the result is uncertain:

```scala
import golem.Guards
import scala.concurrent.Future

val result: Future[Unit] = Guards.withIdempotenceMode(false) {
  // HTTP requests won't be automatically retried
  // Use for non-idempotent external API calls (e.g., payments)
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

## Resource-style Guards

For manual control, use the `use*` / `markAtomicOperation` methods which return a guard. Call `drop()` or `close()` when done:

```scala
import golem.Guards

val guard = Guards.usePersistenceLevel(HostApi.PersistenceLevel.PersistNothing)
// ... do work ...
guard.drop()
```
