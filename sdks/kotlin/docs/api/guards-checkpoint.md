# Guards & Checkpoint

> Scoped runtime controls (`Guards`) and oplog-index revert points (`Checkpoint`), built on the existing `HostApi` oplog/persistence/idempotence primitives — no new WIT surface. **Status:** Complete (`Guards` covers persistence, idempotence, atomic-operation, and retry-policy guards; `Checkpoint` complete).

## Overview

Both types are native ports of the Scala SDK (`Guards.scala`, `Checkpoint.scala`), recast as
**synchronous** Kotlin. The Scala versions wrap each call in a `Future`, but that is a
Scala.js environment artifact — the underlying `HostApi` calls have no async host boundary,
so the faithful native translation is plain synchronous code (no coroutines, no `suspend`).

- **`Guards`** applies a scoped change to the runtime (persistence level, idempotence mode, or
  an atomic region) and guarantees it is restored/closed afterward.
- **`Checkpoint`** captures the current Golem oplog index so execution can be *reverted* back
  to that point — the building block for "try this, and if it doesn't work out, rewind."

## `Guards` API reference

Every `use*` method applies a change and returns a [`Guard`](#guard) that restores the
previous value when [`Guard.drop`](#guard) (or `close()`) is called. Every `with*` method
applies the change for the duration of a `block` and guarantees restoration afterward (even on
exception).

### Persistence level

```kotlin
fun usePersistenceLevel(level: HostApi.PersistenceLevel): PersistenceLevelGuard
fun <A> withPersistenceLevel(level: HostApi.PersistenceLevel, block: () -> A): A
```

Sets the oplog persistence level, restoring the previous level when the guard is dropped /
the block returns.

### Idempotence mode

```kotlin
fun useIdempotenceMode(flag: Boolean): IdempotenceModeGuard
fun <A> withIdempotenceMode(flag: Boolean, block: () -> A): A
```

Enables or disables idempotence mode for the scope, then restores the previous setting.

### Atomic operation

```kotlin
fun markAtomicOperation(): AtomicOperationGuard
fun <A> atomically(block: () -> A): A
```

`markAtomicOperation` opens an atomic region (via `HostApi.markBeginOperation`) and returns a
guard whose `drop()` commits it (`markEndOperation`). `atomically` runs `block` atomically:
on success the region is committed; on failure it calls the host `trap` function, which
surfaces as an **uncatchable** wasm trap (so caller code cannot observe the failure via
`try`/`catch`). The atomic region is intentionally left open on trap — Golem's replay-time
fallback in `markBeginOperation` deletes the partial inner side effects and re-executes the
block.

### Retry policy

```kotlin
fun useRetryPolicy(policy: NamedRetryPolicy): RetryPolicyGuard
fun useRetryPolicy(policy: NamedPolicy): RetryPolicyGuard          // Retry-DSL overload
fun <A> withRetryPolicy(policy: NamedRetryPolicy, block: () -> A): A
fun <A> withRetryPolicy(policy: NamedPolicy, block: () -> A): A    // Retry-DSL overload
```

Registers `policy` as the current agent's retry policy for the scope, then restores the policy
previously registered under the same name when the guard is dropped / the block returns —
removing it if none existed. The `NamedPolicy` overloads accept a policy built with the
[Retry DSL](./retry.md); both delegate to [`RetryApi`](./retry.md).

```kotlin
import cloud.golem.runtime.Guards
import cloud.golem.runtime.host.*   // Retry DSL: Policy, Props, NamedPolicy
import kotlin.time.Duration.Companion.milliseconds

val aggressive = NamedPolicy(
    name = "aggressive",
    policy = Policy.exponential(baseDelay = 50.milliseconds, factor = 2.0).maxRetries(10),
    predicate = Props.statusCode.oneOf(502L, 503L),
)

Guards.withRetryPolicy(aggressive) {
    callFlakyDownstream()   // runs under the aggressive policy; prior policy restored afterward
}
```

### `Guard`

```kotlin
sealed class Guard(private val release: () -> Unit) : AutoCloseable {
    final override fun close()   // = drop()
    fun drop()                   // idempotent: releases once, then no-ops
}

class PersistenceLevelGuard : Guard
class IdempotenceModeGuard   : Guard
class AtomicOperationGuard    : Guard
class RetryPolicyGuard        : Guard
```

`Guard` implements `AutoCloseable`, so a `use*` guard works with Kotlin's `use { }`. `drop()`
is idempotent — releasing an already-released guard does nothing.

## `Checkpoint` API reference

```kotlin
class Checkpoint private constructor(private val oplogIndex: Long)
```

Captures the current oplog index and can revert execution back to it. Construct one with the
[`Checkpoint()` factory](#companion-factories) (`operator fun invoke`) rather than the private
constructor. Reverting uses [`Either`](./transactions.md#either) from the `Transactions`
module.

### Instance members

```kotlin
fun revert(): Nothing
fun <T> unwrapOrRevert(result: Either<*, T>): T
fun <T> runOrRevert(fn: () -> Either<*, T>): T
fun <T> tryOrRevert(fn: () -> T): T
fun assertOrRevert(condition: Boolean)
```

- `revert()` — resets the oplog index to the captured point and never returns normally.
- `unwrapOrRevert(result)` — returns the value of an `Either.Right`, or reverts on
  `Either.Left`.
- `runOrRevert(fn)` — runs `fn`, reverting if it returns `Either.Left`.
- `tryOrRevert(fn)` — runs `fn`, reverting if it **throws**.
- `assertOrRevert(condition)` — reverts if `condition` is `false`.

### Companion factories

```kotlin
operator fun invoke(): Checkpoint
fun <T> withCheckpoint(fn: (Checkpoint) -> Either<*, T>): T
fun <T> withCheckpointTry(fn: (Checkpoint) -> T): T
```

- `Checkpoint()` — creates a checkpoint at the current oplog index.
- `withCheckpoint(fn)` — creates a checkpoint, runs `fn`, and reverts if it returns
  `Either.Left`.
- `withCheckpointTry(fn)` — creates a checkpoint, runs `fn`, and reverts if it throws.

## Examples

### Turning off persistence for a noisy, non-durable side effect

```kotlin
import cloud.golem.runtime.Guards
import cloud.golem.runtime.HostApi

@Agent
class MetricsAgent {
    @Endpoint
    fun recordAndReturn(value: Int): Int =
        // Metrics emission shouldn't bloat the oplog; restore the level afterward.
        Guards.withPersistenceLevel(HostApi.PersistenceLevel.PERSIST_NOTHING) {
            metrics.emit("value", value)
            value * 2
        }
}
```

### Atomic multi-step effect

```kotlin
import cloud.golem.runtime.Guards

@Endpoint
fun transfer(from: String, to: String, amount: Long) {
    Guards.atomically {
        ledger.debit(from, amount)
        ledger.credit(to, amount)
        // If either line traps, the partial effects are discarded and the block re-runs on replay.
    }
}
```

### Manual guard with `use { }`

```kotlin
import cloud.golem.runtime.Guards

@Endpoint
fun withIdempotentScope() {
    Guards.useIdempotenceMode(true).use {   // AutoCloseable -> restored at end of block
        doSomethingThatMayReplay()
    }
}
```

### Checkpoint — rewind on a failed validation

```kotlin
import cloud.golem.runtime.Checkpoint
import cloud.golem.runtime.Either

@Endpoint
fun applyChange(input: String): String =
    Checkpoint.withCheckpoint { _ ->
        val staged = stage(input)                 // side effects recorded in the oplog
        if (staged.isValid) Either.Right(staged.summary)
        else Either.Left("invalid")               // reverts to before stage(...)
    }
```

### Checkpoint — explicit assertion / try variants

```kotlin
import cloud.golem.runtime.Checkpoint

@Endpoint
fun guardedWork(): String {
    val cp = Checkpoint()
    val result = doRiskyWork()
    cp.assertOrRevert(result.isNotEmpty())        // rewind if the postcondition fails
    return result
}

@Endpoint
fun tryWork(): String =
    Checkpoint.withCheckpointTry { _ ->
        callThatMightThrow()                      // any throw rewinds to the checkpoint
    }
```

## Notes

- `Guards` and `Checkpoint` share the same oplog-index mechanism that
  [`Transactions`](./transactions.md) uses; `Transactions` is in fact built on
  `Guards.markAtomicOperation`.
- `atomically` and `Checkpoint.revert` never return normally on their failure paths — a trap
  or an oplog rewind, respectively — so treat them as terminal in control flow.
- `Guard.drop()` is safe to call more than once; `with*` helpers always call it in a `finally`.
- To change retry behaviour, use the [Retry DSL](./retry.md) — the retry guards from the
  Scala SDK are intentionally not part of `Guards` here.

## See also

- [Transactions](./transactions.md) — saga/compensation built on these guards.
- [Retry](./retry.md) — declarative retry policies (replaces the omitted retry guards).
- [Kotlin SDK README](../../README.md)
