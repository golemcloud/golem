# Quota

> Cooperative resource-quota capabilities for Golem agents — the `QuotaApi` binding over `golem:quota/types@1.5.0`. **Status:** Complete.

## Overview

The quota API lets an agent hold an **unforgeable capability** — a `QuotaToken` — that grants
the right to consume a named, rate-limited resource, and to **reserve** capacity from it before
doing work. It is the Kotlin binding for `golem:quota/types@1.5.0`.

The model has three pieces:

1. **`QuotaToken`** — an opaque, *affine* capability for a named resource. It carries no
   readable content, only a handle to an owned host resource. A token can be `split` into a
   child (keeping some capability locally and handing the rest away) and `merge`d back, and it
   can be transferred to exactly one destination — after which it can no longer be used.
2. **`Reservation`** — a short-lived capability obtained via `QuotaToken.reserve(amount)`,
   representing a pending consumption of `amount` units. You then `commit` the *actual* usage.
3. **`QuotaApi`** / `QuotaToken.withReservation` — a scoped helper that reserves, runs your
   block, and commits the usage the block reports (committing zero and rethrowing on failure).

The underlying `quota-token` capability is `golem:core/types@2.0.0`'s `quota-token` resource —
the same resource the SDK's [schema-value tree](types.md) already exposes as
`SchemaValue.QuotaTokenVal`. That resource has **no methods of its own**; this interface exposes
free functions that act on a token handle, plus the `reservation` resource.

### Affine-handle discipline

Both `QuotaToken` and `Reservation` wrap a *nullable* handle and null it out on the operation
that consumes it. This enforces linear/affine use at runtime:

- `QuotaToken.merge(other)` and the SDK's schema-value machinery **take** a token's handle
  (via the internal `take()`), leaving the source token consumed. Any later use traps with a
  clear message telling you to `split` first if you needed to both keep and send the capability.
- `Reservation.commit(used)` nulls the reservation's handle; a second `commit` traps. There is
  no manual `drop` for a reservation — dropping without committing is equivalent to committing
  zero usage (matching the Scala reference).

### The `reservation.commit` shape

`reservation.commit` is declared in WIT as `static func(this: reservation, used: u64)` — a
**static** function that takes the resource by **value** (owned) as an explicit `this`
parameter, not a `[method]`. Calling it *consumes* the reservation: the host releases the
handle as part of the call, so no separate resource-drop is needed after a successful commit.
The SDK models this by taking the `Reservation`'s handle out before the host call.

For the SDK overview see [`../../README.md`](../../README.md). Related: [Types](types.md)
(the `QuotaTokenVal` schema value that transfers a token across an agent boundary).

## API reference

### `QuotaToken`

```kotlin
class QuotaToken {
    fun reserve(amount: Long): Either<FailedReservation, Reservation>
    fun split(childExpectedUse: Long): QuotaToken
    fun merge(other: QuotaToken)
    fun <T> withReservation(amount: Long, block: (Reservation) -> Pair<Long, T>): Either<FailedReservation, T>

    companion object {
        fun create(resourceName: String, expectedUse: Long): QuotaToken
    }
}
```

- **`create(resourceName, expectedUse)`** — request a quota capability for the named resource
  (as declared in the manifest). `expectedUse` is the expected number of units per reservation;
  it derives the credit rate and max-credit used for fair scheduling.
- **`reserve(amount)`** — reserve `amount` units from the local allocation. Blocks internally
  until capacity is available or the resource's enforcement action fires. Returns
  `Either.Right(reservation)` on success, or `Either.Left(FailedReservation)` when the
  enforcement policy is `reject`. Traps (throws) if the token has already been transferred.
- **`split(childExpectedUse)`** — split off a child token carrying `childExpectedUse` units of
  expected-use. The parent's expected-use is reduced by that amount and credits are divided
  proportionally. Traps if `childExpectedUse` exceeds the parent's current expected-use, or if
  the token was already transferred.
- **`merge(other)`** — merge `other` back into this token (combining expected-use and credits).
  `other` is **consumed** and must not be used afterwards; this token stays usable. Traps if the
  tokens refer to different resources, if either was already transferred, or if `other` is `this`.
- **`withReservation(amount, block)`** — convenience wrapper delegating to
  [`QuotaApi.withReservation`](#quotaapi).

### `Reservation`

```kotlin
class Reservation {
    fun commit(used: Long)
}
```

- **`commit(used)`** — commit the *actual* usage. `used` less than reserved returns the unused
  capacity to the pool; `used` greater than reserved deducts the excess as "debt" from the
  token's allocation. Consumes the reservation — a second `commit` traps.

### `FailedReservation`

```kotlin
data class FailedReservation(val estimatedWaitNanos: Long?)
```

Returned as `Either.Left` when a reservation is refused (enforcement policy = `reject`). When
present, `estimatedWaitNanos` is the host's estimate of how long until capacity would be
available.

### `QuotaApi`

```kotlin
object QuotaApi {
    fun <T> withReservation(
        token: QuotaToken,
        amount: Long,
        block: (Reservation) -> Pair<Long, T>,
    ): Either<FailedReservation, T>
}
```

Reserves `amount` units from `token`, runs `block`, then commits the actual usage `block`
returns as the first element of its `Pair` (the second element is the value to propagate). On
any exception thrown by `block`, it commits **zero** usage (returning all capacity to the pool)
and rethrows — so unused capacity is never leaked.

## Examples

All examples assume they run inside a Golem `@Agent` method.

### Reserve, do work, commit actual usage (scoped)

The recommended pattern — `withReservation` handles commit-on-success and commit-zero-on-failure
for you. `block` returns `Pair(actualUnitsUsed, result)`.

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.host.QuotaToken

fun sendBatch(messages: List<String>): Int {
    val token = QuotaToken.create(resourceName = "outbound-emails", expectedUse = 100)

    val result = token.withReservation(amount = messages.size.toLong()) { _ ->
        var sent = 0
        for (m in messages) {
            if (deliver(m)) sent++      // stop early on failure -> commit only what we used
        }
        Pair(sent.toLong(), sent)       // (units actually used, value to return)
    }

    return when (result) {
        is Either.Right -> result.value
        is Either.Left -> {
            val wait = result.value.estimatedWaitNanos
            error("quota rejected; retry in about ${wait ?: 0} ns")
        }
    }
}
```

### Manual reserve / commit

```kotlin
fun chargeOne(token: QuotaToken): Boolean =
    when (val r = token.reserve(amount = 1)) {
        is Either.Left -> false                    // rejected by enforcement policy
        is Either.Right -> {
            val reservation = r.value
            doWork()
            reservation.commit(used = 1)           // consumes the reservation
            true
        }
    }
```

### Split a token to delegate capability, then merge it back

```kotlin
fun delegate(parent: QuotaToken) {
    // Hand 30 units of expected-use to a child; parent keeps the rest and stays usable.
    val child = parent.split(childExpectedUse = 30)

    // ... use `child` locally, or transfer it onward (e.g. as a QuotaTokenVal to another agent) ...

    // Reclaim the child's remaining capability. `child` is consumed by merge.
    parent.merge(child)
    // Using `child` after this point would trap.
}
```

## Notes

- **Affine handles.** A `QuotaToken` can be transferred (via `merge`'s `other`, or the
  schema-value machinery) exactly once; after transfer it traps on any use. If you need to both
  keep and send a capability, `split` first.
- **`reserve` blocks.** It waits internally for capacity; it only returns `Either.Left` when the
  resource's enforcement policy is `reject`.
- **`commit` semantics.** `used < reserved` returns capacity; `used > reserved` incurs debt.
  Committing is mandatory to release a reservation cleanly, but dropping a reservation without
  committing is treated as committing zero.
- **No manual drop.** Neither `Reservation` nor `QuotaToken` exposes a manual drop; consumption
  happens through `commit` / `merge` / transfer, mirroring the Scala reference.
- **`quota-token` has no methods.** All behaviour lives in this interface's free functions;
  the resource itself is the same one surfaced by [`SchemaValue.QuotaTokenVal`](types.md).

See also: [Types](types.md) · [SDK README](../../README.md)
