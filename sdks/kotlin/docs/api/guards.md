# Guards

> Scoped runtime controls (`Guards`) — currently the named **retry-policy** guards. **Status:** Complete for Golem 1.6.0.

## Overview

`Guards` (`cloud.golem.runtime.Guards`) applies a runtime change for a scope and restores the previous
state afterwards. It is a port of the Scala SDK's `Guards`, which in Golem 1.6.0 only has the retry-policy
guards.

> **Removed in 1.6.0.** Golem 1.6.0 removed persistence levels, idempotence-mode switching, atomic regions
> and oplog-index rewinding from the host API, so the corresponding Kotlin members are gone:
> `Guards.usePersistenceLevel` / `withPersistenceLevel`, `useIdempotenceMode` / `withIdempotenceMode`,
> `markAtomicOperation` / `atomically`, and the whole `Checkpoint` class (`revert`, `runOrRevert`, …).
> Use [custom durable invocations](durability.md) to make a side effect replay-safe and
> [saga transactions](transactions.md) for multi-step work with compensation.

The SDK is synchronous throughout, so every function here is a plain function.

## `Guards` API reference

### Retry policy

Members of `object Guards`:

```kotlin
fun useRetryPolicy(policy: NamedRetryPolicy): RetryPolicyGuard
fun useRetryPolicy(policy: NamedPolicy): RetryPolicyGuard          // Retry-DSL overload
fun <A> withRetryPolicy(policy: NamedRetryPolicy, block: () -> A): A
fun <A> withRetryPolicy(policy: NamedPolicy, block: () -> A): A    // Retry-DSL overload
```

Registers `policy` as the current agent's retry policy for the scope, then restores the policy previously
registered under the same name when the guard is dropped / the block returns — removing it if none existed.
The `NamedPolicy` overloads accept a policy built with the [Retry DSL](./retry.md); both delegate to
[`RetryApi`](./retry.md).

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

`Guard` and `RetryPolicyGuard` are nested in `Guards` (`Guards.Guard`, `Guards.RetryPolicyGuard`; the latter has an internal constructor):

```kotlin
sealed class Guard(private val release: () -> Unit) : AutoCloseable {
    final override fun close()   // = drop()
    fun drop()                   // idempotent: releases once, then no-ops
}

class RetryPolicyGuard internal constructor(release: () -> Unit) : Guard(release)
```

`use*` returns a guard you drop yourself (it is `AutoCloseable`, so `use { }` works):

```kotlin
Guards.useRetryPolicy(aggressive).use {
    callFlakyDownstream()
}
```

## Notes

- Retry policies are persisted to the oplog (`set-retry-policy` / `remove-retry-policy`), so a guard's
  registration and its restoration replay deterministically.
- A saga transaction resolves its retry policy with verb `saga`; scope a policy to it with
  `Guards.withRetryPolicy` (see [Transactions](transactions.md)).

## See also

- [Retry](retry.md) — `RetryApi`, the Retry DSL, `resolveRetryPolicy` and `nextRetryDelay`.
- [Transactions](transactions.md), [Durability](durability.md).
