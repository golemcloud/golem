package cloud.golem.runtime

import cloud.golem.runtime.host.NamedPolicy
import cloud.golem.runtime.host.NamedRetryPolicy
import cloud.golem.runtime.host.RetryApi
import cloud.golem.runtime.host.toNamedRetryPolicy

// Native port of Scala's Guards.scala for Golem 1.6.0. Golem 1.6.0 removed persistence levels,
// idempotence-mode switching and atomic regions from the host API (and with them the oplog-index
// rewinding that `Checkpoint` was built on), so the only scoped runtime control left is the named
// retry policy: `useRetryPolicy`/`withRetryPolicy` register a policy for the duration of a scope and
// restore the policy previously registered under the same name afterward (removing it if none
// existed). The SDK is synchronous throughout, so these are plain functions.

/** Scoped runtime controls. Each `use*` applies a change and returns a [Guard] that restores
 * the previous value when [Guard.drop] (or [Guard.close]) is called; each `with*` applies the
 * change for the duration of [block] and guarantees restoration afterward. */
object Guards {
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

    class RetryPolicyGuard internal constructor(release: () -> Unit) : Guard(release)
}
