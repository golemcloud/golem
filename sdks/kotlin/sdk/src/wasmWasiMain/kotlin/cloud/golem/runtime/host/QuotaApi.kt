@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.Either
import cloud.golem.wasm.alloc
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte

// Raw canonical-ABI bindings to golem:quota/types@1.5.0. The `quota-token` capability itself is
// golem:core/types@2.0.0's `quota-token` resource -- the SAME resource
// (SchemaValue.kt's SecretVal/QuotaTokenVal) already established has NO methods of its own; this
// interface only exposes free functions that act on a `quota-token` handle, plus the
// `reservation` resource. Signatures verified via abi-dump's `sig`/`resulttype` modes against
// wit-native/deps/golem-quota/types.wit.
//
// New wrinkle vs everything built so far: `reservation.commit` is declared
// `static func(this: reservation, used: u64)` -- a static function that takes the resource by
// VALUE (owned, not borrowed) as an explicit `this` parameter, not a `[method]`. Calling it
// consumes the reservation (the host releases the handle as part of the call); there is no
// separate [resource-drop]reservation call needed after a successful commit. Same
// `[static]<resource>.<name>` raw name shape as `[static]bucket.open-bucket`, just with
// the resource threaded as a normal parameter instead of being implicit.
@kotlin.wasm.WasmImport("golem:quota/types@1.5.0", "new-token")
private external fun hostNewToken(namePtr: Int, nameLen: Int, expectedUse: Long): Int

@kotlin.wasm.WasmImport("golem:quota/types@1.5.0", "reserve")
private external fun hostReserve(tokenHandle: Int, amount: Long, retPtr: Int)

@kotlin.wasm.WasmImport("golem:quota/types@1.5.0", "split")
private external fun hostSplit(tokenHandle: Int, childExpectedUse: Long): Int

@kotlin.wasm.WasmImport("golem:quota/types@1.5.0", "merge")
private external fun hostMerge(tokenHandle: Int, otherHandle: Int)

@kotlin.wasm.WasmImport("golem:quota/types@1.5.0", "[static]reservation.commit")
private external fun hostReservationCommit(reservationHandle: Int, used: Long)

private const val TOKEN_CONSUMED =
    "quota token has already been transferred and can no longer be used; split the token first " +
        "if you need to both keep and send a capability"

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/** Returned when a reservation cannot be granted (enforcement policy = `reject`). */
data class FailedReservation(val estimatedWaitNanos: Long?)

/**
 * A short-lived capability representing a pending resource consumption. Dropping without
 * calling [commit] is equivalent to committing zero usage -- this SDK doesn't expose a manual
 * drop for it (matching the Scala reference), only [commit], which itself consumes the handle.
 */
class Reservation internal constructor(handle: Int) {
    private var handle: Int? = handle

    /**
     * Commit actual usage. `used` < reserved returns unused capacity to the pool; `used` >
     * reserved deducts the excess as "debt" from the token's allocation. Consumes this
     * reservation -- calling [commit] twice fails.
     */
    fun commit(used: Long) {
        val h = handle ?: error("reservation already committed")
        handle = null
        hostReservationCommit(h, used)
    }
}

/**
 * An unforgeable capability granting the right to consume a named resource. Holds only an
 * opaque, affine handle to the owned host resource: it carries no readable content and can be
 * transferred to exactly one destination ([merge]'s `other` argument, or wherever this SDK's
 * schema-value-tree machinery sends a [cloud.golem.runtime.SchemaValue.QuotaTokenVal]). Once
 * transferred, the token can no longer be used -- [split] first if you need to both keep and
 * send a capability.
 */
class QuotaToken internal constructor(handle: Int) {
    private var handle: Int? = handle

    private fun <T> withHandle(block: (Int) -> T): T = block(handle ?: error(TOKEN_CONSUMED))

    /** Takes ownership of this token's handle for a one-time transfer (e.g. into [merge]'s `other`). Returns null if already consumed. */
    internal fun take(): Int? {
        val h = handle
        handle = null
        return h
    }

    /**
     * Reserve `amount` units from the local allocation. Blocks internally until capacity is
     * available or the resource's enforcement action fires. Returns `Right(reservation)` on
     * success, or `Left(FailedReservation)` when the enforcement policy is `reject`. Traps (via
     * [error]) if this token has already been transferred.
     */
    fun reserve(amount: Long): Either<FailedReservation, Reservation> = withHandle { h ->
        // result<reservation, failed-reservation>: tag@0(1,1), payload@8 (max(reservation i32/4,
        // failed-reservation{option<u64>} 16/8) = 16) -> 24 total, align8.
        val retPtr = alloc(24, 8)
        hostReserve(h, amount, retPtr)
        if (loadByte(retPtr).toInt() == 0) {
            Either.Right(Reservation(loadInt(retPtr + 8)))
        } else {
            val errBase = retPtr + 8 // failed-reservation: {estimated-wait-nanos: option<u64>} @0
            val hasWait = loadByte(errBase).toInt() != 0
            Either.Left(FailedReservation(if (hasWait) loadLong(errBase + 8) else null))
        }
    }

    /**
     * Split off a child token with `childExpectedUse` units of expected-use. The parent's
     * expected-use is reduced by `childExpectedUse`; credits are divided proportionally. Traps
     * if `childExpectedUse` exceeds the parent's current expected-use, or if this token has
     * already been transferred.
     */
    fun split(childExpectedUse: Long): QuotaToken = withHandle { h -> QuotaToken(hostSplit(h, childExpectedUse)) }

    /**
     * Merge `other` back into this token: combines expected-use and credits. `other` is
     * consumed by this call and must not be used afterwards; this token remains usable. Traps
     * if the tokens refer to different resources, or if either token has already been
     * transferred.
     */
    fun merge(other: QuotaToken) {
        require(other !== this) { "cannot merge a quota token with itself" }
        withHandle { thisHandle ->
            val otherHandle = other.take() ?: error(TOKEN_CONSUMED)
            hostMerge(thisHandle, otherHandle)
        }
    }

    /** Reserve `amount` units, run [block], then commit the actual usage [block] returns. Commits zero usage and rethrows on failure. */
    fun <T> withReservation(amount: Long, block: (Reservation) -> Pair<Long, T>): Either<FailedReservation, T> = QuotaApi.withReservation(this, amount, block)

    companion object {
        /**
         * Request a quota capability for the named resource.
         *
         * @param resourceName the resource name as declared in the manifest.
         * @param expectedUse expected units per reservation; used to derive the credit rate and max-credit for fair scheduling.
         */
        fun create(resourceName: String, expectedUse: Long): QuotaToken {
            val (namePtr, nameLen) = lowerStringToPtrLen(resourceName)
            return QuotaToken(hostNewToken(namePtr, nameLen, expectedUse))
        }
    }
}

object QuotaApi {
    /**
     * Reserve `amount` units from `token`, run [block], then commit the actual usage [block]
     * returns. Commits zero usage and rethrows on failure, ensuring unused capacity is always
     * returned to the pool.
     */
    fun <T> withReservation(token: QuotaToken, amount: Long, block: (Reservation) -> Pair<Long, T>): Either<FailedReservation, T> = when (val r = token.reserve(amount)) {
        is Either.Left -> Either.Left(r.value)
        is Either.Right -> {
            val reservation = r.value
            try {
                val (used, value) = block(reservation)
                reservation.commit(used)
                Either.Right(value)
            } catch (e: Throwable) {
                reservation.commit(0L)
                throw e
            }
        }
    }
}
