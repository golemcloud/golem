package cloud.golem.runtime

import cloud.golem.runtime.host.NamedPolicy
import cloud.golem.runtime.host.NamedRetryPolicy
import cloud.golem.runtime.host.RetryApi
import cloud.golem.runtime.host.toNamedRetryPolicy

// Native port of Scala's Guards.scala. Unlike the Scala.js version (which wraps every guard in
// a Future -- an artifact of Scala.js's single-threaded, callback-based environment), this SDK's
// underlying HostApi calls are all plain synchronous functions with no async host boundary at
// all, so the native port is synchronous throughout: no coroutines, no suspend. Matches
// CLAUDE.md's own guidance ("map suspend -> Golem durable suspension where the host model
// allows") -- here the host model doesn't involve async at all, so a synchronous translation is
// the faithful one, not a simplification.
//
// `useRetryPolicy`/`withRetryPolicy` mirror Scala's Guards: they register a named retry policy for
// the duration of a scope and restore the policy previously registered under the same name
// afterward (removing it if none existed). They build directly on `RetryApi` (the host binding)
// and the Retry DSL's `NamedPolicy` -- both fully ported -- so no blocker remains.

/** Scoped runtime controls. Each `use*` applies a change and returns a [Guard] that restores
 * the previous value when [Guard.drop] (or [Guard.close]) is called; each `with*` applies the
 * change for the duration of [block] and guarantees restoration afterward. */
object Guards {
    fun usePersistenceLevel(level: HostApi.PersistenceLevel): PersistenceLevelGuard {
        val original = HostApi.getOplogPersistenceLevel()
        HostApi.setOplogPersistenceLevel(level)
        return PersistenceLevelGuard { HostApi.setOplogPersistenceLevel(original) }
    }

    fun <A> withPersistenceLevel(level: HostApi.PersistenceLevel, block: () -> A): A {
        val guard = usePersistenceLevel(level)
        try {
            return block()
        } finally {
            guard.drop()
        }
    }

    fun useIdempotenceMode(flag: Boolean): IdempotenceModeGuard {
        val original = HostApi.getIdempotenceMode()
        HostApi.setIdempotenceMode(flag)
        return IdempotenceModeGuard { HostApi.setIdempotenceMode(original) }
    }

    fun <A> withIdempotenceMode(flag: Boolean, block: () -> A): A {
        val guard = useIdempotenceMode(flag)
        try {
            return block()
        } finally {
            guard.drop()
        }
    }

    fun markAtomicOperation(): AtomicOperationGuard {
        val begin = HostApi.markBeginOperation()
        return AtomicOperationGuard { HostApi.markEndOperation(begin) }
    }

    /**
     * Executes [block] atomically. On success the atomic region is committed via
     * `markEndOperation`. On failure, calls the host `trap` function, which surfaces as an
     * uncatchable wasm trap so caller code cannot observe the failure via `try`/`catch`. The
     * atomic region is intentionally left open -- the existing replay-time fallback in
     * `markBeginOperation` deletes the partial inner side effects and re-executes the block.
     */
    fun <A> atomically(block: () -> A): A {
        val guard = markAtomicOperation()
        return try {
            val result = block()
            guard.drop()
            result
        } catch (e: Throwable) {
            HostApi.trap("atomic block failed: ${e.stackTraceToString()}")
        }
    }

    /**
     * Registers [policy] as the current agent's retry policy, returning a [RetryPolicyGuard] that
     * on [Guard.drop] (or [Guard.close]) restores the policy previously registered under the same
     * name -- or removes it if none existed. Faithful port of Scala's `Guards.useRetryPolicy`.
     */
    fun useRetryPolicy(policy: NamedRetryPolicy): RetryPolicyGuard {
        val previous = RetryApi.getRetryPolicyByName(policy.name)
        val name = policy.name
        RetryApi.setRetryPolicy(policy)
        return RetryPolicyGuard {
            if (previous != null) RetryApi.setRetryPolicy(previous) else RetryApi.removeRetryPolicy(name)
        }
    }

    /** DSL overload of [useRetryPolicy] taking a [NamedPolicy] built with the Retry DSL. */
    fun useRetryPolicy(policy: NamedPolicy): RetryPolicyGuard = useRetryPolicy(policy.toNamedRetryPolicy())

    /** Registers [policy] for the duration of [block], restoring the previous policy afterward. */
    fun <A> withRetryPolicy(policy: NamedRetryPolicy, block: () -> A): A {
        val guard = useRetryPolicy(policy)
        try {
            return block()
        } finally {
            guard.drop()
        }
    }

    /** DSL overload of [withRetryPolicy] taking a [NamedPolicy] built with the Retry DSL. */
    fun <A> withRetryPolicy(policy: NamedPolicy, block: () -> A): A = withRetryPolicy(policy.toNamedRetryPolicy(), block)

    sealed class Guard(private val release: () -> Unit) : AutoCloseable {
        private var active = true
        final override fun close() = drop()
        fun drop() {
            if (active) {
                active = false
                release()
            }
        }
    }

    class PersistenceLevelGuard internal constructor(release: () -> Unit) : Guard(release)
    class IdempotenceModeGuard internal constructor(release: () -> Unit) : Guard(release)
    class AtomicOperationGuard internal constructor(release: () -> Unit) : Guard(release)
    class RetryPolicyGuard internal constructor(release: () -> Unit) : Guard(release)
}
