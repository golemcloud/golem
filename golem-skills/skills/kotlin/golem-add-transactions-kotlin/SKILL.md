---
name: golem-add-transactions-kotlin
description: "Adding saga-style transactions with compensation to a Kotlin agent. Use when the user asks for rollback, compensating actions, multi-step workflows that must be undone on failure, or retrying a saga until it succeeds."
---

# Saga Transactions (Kotlin)

Golem 1.6.0 has **sagas**, not atomic regions: attempts and compensations append to the oplog and nothing rewinds. `Transactions` (in `cloud.golem.runtime`) pairs each side-effecting operation with a compensation that undoes it.

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.TransactionFailure
import cloud.golem.runtime.Transactions

val reserve = Transactions.operation<Order, Reservation, String>(
    run = { order -> reserveStock(order) },                 // (In) -> Either<Err, Out>
    compensate = { order, res -> releaseStock(order, res) }, // (In, Out) -> Either<Err, Unit>
)

val result = Transactions.fallibleTransaction<Receipt, String> { tx ->
    val reserved = tx.execute(reserve, order)
    if (reserved is Either.Left) return@fallibleTransaction reserved
    tx.execute(charge, order)               // if this fails, `reserve` is compensated
}

when (result) {
    is Either.Right -> ok(result.value)
    is Either.Left -> when (val f = result.value) {
        is TransactionFailure.FailedAndRolledBackCompletely -> clean(f.error)
        is TransactionFailure.FailedAndRolledBackPartially -> manualCleanup(f.error, f.compensationFailure)
        is TransactionFailure.RetryExhausted -> giveUp(f.error)                  // infallibleTransaction only
        is TransactionFailure.RetryPolicyError -> giveUp(f.failure)              // host rejected the saga policy
    }
}
```

`ok`, `clean`, `manualCleanup`, `giveUp`, `reserveStock`, `releaseStock`, `charge` and `order` are your own code; `Operation<In, Out, Err>` and `Either` are in `cloud.golem.runtime`.

## Flavours

- `fallibleTransaction` — on failure, compensations run in reverse order (best effort) and you get the failure with its rollback status.
- `infallibleTransaction` — after each fully compensated failure the host evaluates the retry policy (verb `saga`, the agent's `urn:worker:...` noun, properties `attempt` and `error-kind` = `operation-error`) and either waits the delay (a durable sleep) and runs a new attempt, or ends in `TransactionFailure.RetryExhausted` (`RetryPolicyError` if the host rejects the policy). A partial rollback ends the saga without another attempt. Scope a policy with `cloud.golem.runtime.Guards.withRetryPolicy(NamedPolicy(...)) { ... }` (see `golem-retry-policies-kotlin`).

## Rules

- The body returns `Either<Err, A>`; return `Left` for expected failures. **Exceptions** are programming errors: they propagate without compensation.
- If a compensation fails, the rollback stops (`FailedAndRolledBackPartially`).
- Compensation is best effort and gives no isolation from concurrent work.
- Make non-deterministic side effects replay-safe with `cloud.golem.runtime.host.DurabilityApi.durable(...)` (`sdks/kotlin/docs/api/durability.md`).

## Related skills

- `golem-retry-policies-kotlin`
