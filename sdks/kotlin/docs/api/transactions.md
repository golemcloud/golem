# Transactions (Saga / Compensation)

> Compensating-transaction (saga) helpers for Golem agents, built entirely on existing host primitives (`getOplogIndex`/`setOplogIndex`, `markAtomicOperation`, `trap`) — no new WIT surface. **Status:** Complete.

## Overview

`Transactions` is pure application logic layered over the Golem host. It lets an agent
run a sequence of side-effecting **operations**, each paired with a **compensation** that
undoes it, so that a mid-sequence failure can roll the whole thing back cleanly. This is the
classic *saga* pattern.

Two flavours are provided:

- **Infallible** ([`infallibleTransaction`](#infallibletransaction)) — for work that *must*
  eventually succeed. On any failure, every registered compensation runs in reverse order,
  the oplog index is reset to the transaction's start, and the whole body re-executes.
  It keeps retrying until all operations succeed.
- **Fallible** ([`fallibleTransaction`](#fallibletransaction)) — for work where you want to
  *handle* the error yourself. On failure, compensations run best-effort (no retry) and the
  failure is returned as a [`TransactionFailure`](#transactionfailure).

### Execution model — synchronous, no async boundary

This is a native port of the Scala SDK's `Transactions.scala`. The Scala version wraps every
step in a `Future`, but that is an artifact of its Scala.js single-threaded environment. The
native SDK's underlying host calls (`getOplogIndex`, `setOplogIndex`, `markBeginOperation`,
`markEndOperation`, `trap`) have **no async host boundary at all**, so the port is
**synchronous throughout** — no coroutines, no `suspend`. A synchronous translation is the
faithful one here, not a simplification.

Rollback / retry works by moving the Golem oplog index: on failure the index is reset to the
value captured at the start of the transaction, which is how re-execution and durable replay
are achieved. See also [Guards & Checkpoint](./guards-checkpoint.md) for the lower-level
primitives this builds on.

## API reference

### `Either`

A minimal local `Either` — this SDK has no Arrow/stdlib `Either` dependency; `Transactions`
introduces it (and [`Checkpoint`](./guards-checkpoint.md) reuses it).

```kotlin
sealed class Either<out L, out R> {
    data class Left<out L>(val value: L) : Either<L, Nothing>()
    data class Right<out R>(val value: R) : Either<Nothing, R>()
}
```

By convention `Left` carries an error and `Right` carries a success value.

### `Operation`

An atomic step with an execute half and a compensate half.

```kotlin
class Operation<In, Out, Err>(
    private val run: (In) -> Either<Err, Out>,
    private val compensateFn: (In, Out) -> Either<Err, Unit>,
) {
    fun execute(input: In): Either<Err, Out>
    fun compensate(input: In, output: Out): Either<Err, Unit>
}
```

Construct one with the [`Transactions.operation`](#transactionsoperation) factory rather than
the constructor directly.

### `TransactionFailure`

Describes how a fallible transaction failed.

```kotlin
sealed class TransactionFailure<out Err> {
    data class FailedAndRolledBackCompletely<Err>(val error: Err) : TransactionFailure<Err>()
    data class FailedAndRolledBackPartially<Err>(val error: Err, val compensationFailure: Err) : TransactionFailure<Err>()
}
```

- `FailedAndRolledBackCompletely` — the operation failed and *every* compensation succeeded.
- `FailedAndRolledBackPartially` — the operation failed and, while rolling back, a
  compensation itself failed (`compensationFailure` carries that error; remaining
  compensations are not attempted).

### `Transactions.operation`

```kotlin
fun <In, Out, Err> operation(
    run: (In) -> Either<Err, Out>,
    compensate: (In, Out) -> Either<Err, Unit>,
): Operation<In, Out, Err>
```

Builds an [`Operation`](#operation) from an execute function and a compensate function. The
compensate function receives both the original `input` and the `output` the execute step
produced, so it has everything needed to undo the effect.

### `Transactions.infallibleTransaction`

```kotlin
fun <A> infallibleTransaction(body: (InfallibleTransaction) -> A): A
```

Runs `body` inside an atomic region. Call
[`InfallibleTransaction.execute`](#infallibletransactionexecute) for each step. If any step
fails, all registered compensations run in reverse order, the oplog index resets to the
transaction start, and `body` re-runs from the top — repeating until it completes without
failure. An unexpected exception (anything other than the internal retry signal) traps.

#### `InfallibleTransaction.execute`

```kotlin
fun <In, Out, Err> execute(operation: Operation<In, Out, Err>, input: In): Out
```

Executes `operation` with `input`. On success it registers the compensation and returns the
raw `Out` value (no `Either` to unwrap — failures roll back and retry rather than being
returned). On failure it triggers rollback: all compensations registered so far run in
reverse order, then a retry is signalled.

### `Transactions.fallibleTransaction`

```kotlin
fun <A, Err> fallibleTransaction(
    body: (FallibleTransaction<Err>) -> Either<Err, A>
): Either<TransactionFailure<Err>, A>
```

Runs `body` inside an atomic region and returns its result. If `body` returns `Either.Right`,
the transaction commits and the value is returned as `Right`. If it returns `Either.Left`,
registered compensations run in reverse order (best-effort) and a
[`TransactionFailure`](#transactionfailure) is returned as `Left`. Unlike the infallible
variant, there is no automatic retry.

#### `FallibleTransaction.execute`

```kotlin
fun <In, Out> execute(operation: Operation<In, Out, Err>, input: In): Either<Err, Out>
```

Executes `operation`. On success it registers the compensation and returns `Right(out)`. On
failure it returns the error as `Left` **without** rolling back — you decide how to proceed.
Typically you propagate the `Left` out of `body`, which triggers the rollback in
`fallibleTransaction`.

#### `FallibleTransaction.onFailure`

```kotlin
fun onFailure(error: Err): TransactionFailure<Err>
```

Runs all registered compensations in reverse order and classifies the outcome as
`FailedAndRolledBackCompletely` or `FailedAndRolledBackPartially`. `fallibleTransaction`
calls this for you when `body` returns `Left`; call it directly only if you manage the
`FallibleTransaction` yourself.

## Examples

### Infallible saga — book a trip, retry until it sticks

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.Transactions

@Agent
class TripBookingAgent {

    private val reserveFlight = Transactions.operation<String, String, String>(
        run = { city -> Either.Right(flightApi.reserve(city)) },       // -> reservationId
        compensate = { _, reservationId -> Either.Right(flightApi.cancel(reservationId)) },
    )

    private val chargeCard = Transactions.operation<Int, String, String>(
        run = { cents -> Either.Right(payments.charge(cents)) },        // -> chargeId
        compensate = { _, chargeId -> Either.Right(payments.refund(chargeId)) },
    )

    @Endpoint
    fun bookTrip(city: String, priceCents: Int): String =
        Transactions.infallibleTransaction { tx ->
            // execute returns the raw success value; a failure rolls back + retries the whole body.
            val reservationId = tx.execute(reserveFlight, city)
            val chargeId = tx.execute(chargeCard, priceCents)
            "booked $reservationId / paid $chargeId"
        }
}
```

If `chargeCard` fails, the flight reservation's compensation (`flightApi.cancel`) runs, the
oplog rewinds, and the whole body re-executes — so a transient payment outage is retried
transparently.

### Fallible saga — return a typed failure instead of retrying

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.TransactionFailure
import cloud.golem.runtime.Transactions

@Endpoint
fun tryBookTrip(city: String, priceCents: Int): String {
    val result = Transactions.fallibleTransaction<String, String> { tx ->
        val reservationId = when (val r = tx.execute(reserveFlight, city)) {
            is Either.Right -> r.value
            is Either.Left -> return@fallibleTransaction Either.Left(r.value) // rolls back
        }
        when (val c = tx.execute(chargeCard, priceCents)) {
            is Either.Right -> Either.Right("booked $reservationId / paid ${c.value}")
            is Either.Left -> Either.Left(c.value)                             // rolls back reservation
        }
    }

    return when (result) {
        is Either.Right -> result.value
        is Either.Left -> when (val f = result.value) {
            is TransactionFailure.FailedAndRolledBackCompletely ->
                "failed, fully rolled back: ${f.error}"
            is TransactionFailure.FailedAndRolledBackPartially ->
                "failed, PARTIAL rollback: ${f.error} (compensation also failed: ${f.compensationFailure})"
        }
    }
}
```

## Notes

- Compensations are always run in **reverse registration order** (last operation undone first).
- **Infallible** `execute` returns `Out` directly; **fallible** `execute` returns
  `Either<Err, Out>`. That difference reflects retry-vs-return semantics.
- Returning `Either.Left` from a fallible `body` is what actually triggers rollback — a
  successfully-`execute`d step is *not* undone unless the body ultimately signals failure.
- The whole transaction runs inside an atomic operation guard (see
  [`Guards.markAtomicOperation`](./guards-checkpoint.md)); the guard is always dropped, even
  on the failure/retry paths.
- Unexpected exceptions (not the internal retry signal) are surfaced via the host `trap`
  function, which appears as an uncatchable wasm trap.

## See also

- [Guards & Checkpoint](./guards-checkpoint.md) — the atomic-region and oplog-rewind
  primitives this builds on.
- [Retry](./retry.md) — declarative, host-level retry policies (a different mechanism from
  saga compensation).
- [Kotlin SDK README](../../README.md)
