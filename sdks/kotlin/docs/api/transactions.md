# Transactions (Saga / Compensation)

> Compensating-transaction (saga) helpers for Golem agents, ported from the Scala/Rust SDKs for Golem 1.6.0. **Status:** Complete.

## Overview

`Transactions` lets an agent run a sequence of side-effecting **operations**, each paired with a
**compensation** that undoes it, so a mid-sequence failure can be rolled back cleanly — the classic *saga*
pattern.

> **Changed in Golem 1.6.0.** These helpers are sagas, **not atomic regions**. Golem 1.6.0 removed atomic
> regions and oplog-index rewinding, so a failed attempt is never "rewound": attempts and compensations
> append to the oplog, they do not undo concurrent work, and successful steps compensate in reverse order.
> The API changed accordingly: the body returns `Either<Err, A>` for both flavours (it no longer throws a
> retry signal), results are `Either<TransactionFailure<Err>, A>`, and `FallibleTransaction` is a top-level
> class used by both.

Two flavours:

- **Fallible** ([`fallibleTransaction`](#fallibletransaction)) — handle the error yourself. On failure the
  compensations run (best-effort, reverse order) and the failure is returned with its rollback status.
- **Infallible** ([`infallibleTransaction`](#infallibletransaction)) — keep trying. After each *fully
  compensated* failure the host resolves a retry policy and either returns a delay to wait before the next
  attempt or reports the policy exhausted.

The SDK is synchronous: no coroutines, no `suspend`.

## API reference

```kotlin
// cloud.golem.runtime.Either
sealed class Either<out L, out R> { data class Left<out L>(val value: L); data class Right<out R>(val value: R) }

class Operation<In, Out, Err>(run: (In) -> Either<Err, Out>, compensateFn: (In, Out) -> Either<Err, Unit>) {
    fun execute(input: In): Either<Err, Out>
    fun compensate(input: In, output: Out): Either<Err, Unit>
}

class FallibleTransaction<Err> internal constructor() {
    /** Runs the operation; on success registers its compensation. */
    fun <In, Out> execute(operation: Operation<In, Out, Err>, input: In): Either<Err, Out>
}

sealed class TransactionFailure<out Err> {
    data class FailedAndRolledBackCompletely<Err>(val error: Err)
    data class FailedAndRolledBackPartially<Err>(val error: Err, val compensationFailure: Err)
    data class RetryExhausted<Err>(val error: Err)                       // infallible: policy exhausted
    data class RetryPolicyError<Err>(val failure: Err, val message: String) // infallible: host rejected the policy
}

object Transactions {
    fun <In, Out, Err> operation(run, compensate): Operation<In, Out, Err>
    fun <A, Err> fallibleTransaction(body: (FallibleTransaction<Err>) -> Either<Err, A>): Either<TransactionFailure<Err>, A>
    fun <A, Err> infallibleTransaction(attemptsMax: Long = 10_000, body: (FallibleTransaction<Err>) -> Either<Err, A>): Either<TransactionFailure<Err>, A>
}
```

### `fallibleTransaction`

```kotlin
val result = Transactions.fallibleTransaction<Int, String> { tx ->
    val reserved = tx.execute(reserveStock, order)
    if (reserved is Either.Left) return@fallibleTransaction reserved
    tx.execute(chargeCard, order)          // fails -> reserveStock is compensated
}
when (result) {
    is Either.Right -> ok(result.value)
    is Either.Left -> when (val f = result.value) {
        is TransactionFailure.FailedAndRolledBackCompletely -> cleanFailure(f.error)
        is TransactionFailure.FailedAndRolledBackPartially -> needsManualCleanup(f.error, f.compensationFailure)
        else -> error("unreachable for a fallible transaction")
    }
}
```

If a compensation itself fails, the rollback stops there (`FailedAndRolledBackPartially`); steps registered
before it are *not* compensated.

### `infallibleTransaction`

Runs new attempts of the body until it succeeds, the policy is exhausted, or `attemptsMax` (1 to 10 000) attempts have failed
(`RetryExhausted`). After each fully compensated failure the host evaluates the
retry policy for:

| | |
|---|---|
| verb | `saga` |
| noun | `urn:worker:<component-uuid>/<agent-id>` |
| properties | `attempt` (integer, 1 for the first failure) and `error-kind` (`"operation-error"`) |

The host returns a delay (the helper waits it with a durable `wasi:clocks/monotonic-clock` `wait-for`) or
reports the policy exhausted → `RetryExhausted`. If no policy matches, the host's default retry
configuration is used. The previous opaque continuation state is threaded through each evaluation and
the elapsed monotonic time since the first attempt is passed along. Partial compensation stops the saga
without another attempt (`FailedAndRolledBackPartially`); a rejected policy or continuation is
`RetryPolicyError`.

Scope a policy to sagas with `Guards.withRetryPolicy` (see [Retry](retry.md)):

```kotlin
val result = Guards.withRetryPolicy(NamedPolicy("saga-backoff", Policy.exponential(100.milliseconds, 2.0).maxRetries(5))) {
    Transactions.infallibleTransaction<Int, String> { tx ->
        val a = tx.execute(createResource, input)
        if (a is Either.Left) a else tx.execute(flakyStep, (a as Either.Right).value)
    }
}
```

> Exceptions thrown by the body are programming errors: they propagate **without** compensation (the
> invocation fails and Golem's own retry handling applies). Return `Either.Left` for expected failures.

## Notes

- Compensation is best-effort and **not** transactional isolation: nothing rolls back concurrent work.
- Verified against the 1.6.0 worker executor: an infallible transaction whose third attempt succeeds
  compensates twice and waits the policy's delay between attempts; a one-retry policy ends in `RetryExhausted`.

## See also

- [Durability](durability.md) — make an individual side effect replay-safe.
- [Retry](retry.md), [Guards](guards.md), [Host API](host-api.md).
