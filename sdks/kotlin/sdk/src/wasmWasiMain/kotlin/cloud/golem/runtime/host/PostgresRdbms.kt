@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.Either
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadDouble
import cloud.golem.wasm.loadFloat
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.loadShort
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeDouble
import cloud.golem.wasm.storeFloat
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.storeShort

// Raw canonical-ABI bindings to golem:rdbms/postgres@1.5.0 (Postgres only, primitive db-value cases only; see PostgresDbValue's doc comment
// for exactly what's deferred and why). golem:rdbms/mysql@1.5.0 and golem:rdbms/ignite2@1.5.0
// are entirely out of scope for this increment; they mirror the same shape (their own
// large db-value variant + Connection/Transaction resources) and are future increments of
// their own, matching how this whole SDK stages large per-backend/per-file surfaces.
//
// db-connection.open and db-transaction's methods are all ordinary resource-ABI shapes already
// proven elsewhere in this SDK ([static]<resource>.<name> for open, [method] for the rest).
// Signatures verified via abi-dump's `sig`/`resulttype` modes against
// wit-native/deps/golem-rdbms/{types,postgres}.wit.

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[static]db-connection.open")
private external fun hostConnectionOpen(addrPtr: Int, addrLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-connection.query")
private external fun hostConnectionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-connection.execute")
private external fun hostConnectionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-connection.begin-transaction")
private external fun hostConnectionBeginTransaction(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[resource-drop]db-connection")
private external fun hostConnectionDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-transaction.query")
private external fun hostTransactionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-transaction.execute")
private external fun hostTransactionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-transaction.commit")
private external fun hostTransactionCommit(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]db-transaction.rollback")
private external fun hostTransactionRollback(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[resource-drop]db-transaction")
private external fun hostTransactionDrop(handle: Int)

// lazy-db-value: the recursion db-value's composite/domain/array/range cases go through.
// [constructor]lazy-db-value has indirect_params=true (confirmed via abi-dump's funcargs mode)
// -- its single `value: db-value` param doesn't fit the flat-param threshold, so the guest
// builds the FULL db-value in memory (same layout lowerDbValue already writes) and passes a
// POINTER to it as the function's sole argument, with the resource handle returned directly
// (no retptr). This is why db-value can have a bounded size (56 bytes) despite being
// conceptually recursive: the recursion is INDIRECT, through resource handles in the host's
// own table, not inlined in memory -- `list<lazy-db-value>` at the wire level is just
// `list<i32>` (a list of handles), and reading a nested value means calling .get() on each
// handle, not walking inline bytes.
@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[constructor]lazy-db-value")
private external fun hostLazyDbValueConstructor(valuePtr: Int): Int

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[method]lazy-db-value.get")
private external fun hostLazyDbValueGet(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/postgres@1.5.0", "[resource-drop]lazy-db-value")
private external fun hostLazyDbValueDrop(handle: Int)

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/** Matches `golem:rdbms/types@1.5.0`'s `date` record (8 bytes, align 4). */
data class DbDate(val year: Int, val month: UByte, val day: UByte)

/** Matches `golem:rdbms/types@1.5.0`'s `time` record (8 bytes, align 4). */
data class DbTime(val hour: UByte, val minute: UByte, val second: UByte, val nanosecond: UInt)

/** Matches `golem:rdbms/types@1.5.0`'s `timestamp` record (16 bytes, align 4). */
data class DbTimestamp(val date: DbDate, val time: DbTime)

/** Matches `golem:rdbms/types@1.5.0`'s `timestamptz` record (20 bytes, align 4). */
data class DbTimestampTz(val timestamp: DbTimestamp, val offset: Int)

/** Matches `golem:rdbms/types@1.5.0`'s `timetz` record (12 bytes, align 4). */
data class DbTimeTz(val time: DbTime, val offset: Int)

/** Matches `golem:rdbms/postgres@1.5.0`'s `interval` record (16 bytes, align 8). */
data class DbInterval(val months: Int, val days: Int, val microseconds: Long)

internal fun liftDbDate(base: Int): DbDate = DbDate(loadInt(base), loadByte(base + 4).toUByte(), loadByte(base + 5).toUByte())
internal fun liftDbTime(base: Int): DbTime = DbTime(loadByte(base).toUByte(), loadByte(base + 1).toUByte(), loadByte(base + 2).toUByte(), loadInt(base + 4).toUInt())
internal fun liftDbTimestamp(base: Int): DbTimestamp = DbTimestamp(liftDbDate(base), liftDbTime(base + 8))
private fun liftDbTimestampTz(base: Int): DbTimestampTz = DbTimestampTz(liftDbTimestamp(base), loadInt(base + 16))
private fun liftDbTimeTz(base: Int): DbTimeTz = DbTimeTz(liftDbTime(base), loadInt(base + 8))
private fun liftDbInterval(base: Int): DbInterval = DbInterval(loadInt(base), loadInt(base + 4), loadLong(base + 8))

internal fun storeDbDate(base: Int, d: DbDate) {
    storeInt(base, d.year)
    storeByte(base + 4, d.month.toByte())
    storeByte(base + 5, d.day.toByte())
}
internal fun storeDbTime(base: Int, t: DbTime) {
    storeByte(base, t.hour.toByte())
    storeByte(base + 1, t.minute.toByte())
    storeByte(base + 2, t.second.toByte())
    storeInt(base + 4, t.nanosecond.toInt())
}
internal fun storeDbTimestamp(base: Int, ts: DbTimestamp) {
    storeDbDate(base, ts.date)
    storeDbTime(base + 8, ts.time)
}
private fun storeDbTimestampTz(base: Int, tstz: DbTimestampTz) {
    storeDbTimestamp(base, tstz.timestamp)
    storeInt(base + 16, tstz.offset)
}
private fun storeDbTimeTz(base: Int, ttz: DbTimeTz) {
    storeDbTime(base, ttz.time)
    storeInt(base + 8, ttz.offset)
}
private fun storeDbInterval(base: Int, iv: DbInterval) {
    storeInt(base, iv.months)
    storeInt(base + 4, iv.days)
    storeLong(base + 8, iv.microseconds)
}

/** Matches `golem:rdbms/types@1.5.0`'s `ip-address` variant (18 bytes, align 2). */
sealed class IpAddress {
    data class Ipv4(val a: UByte, val b: UByte, val c: UByte, val d: UByte) : IpAddress()
    data class Ipv6(
        val a: UShort,
        val b: UShort,
        val c: UShort,
        val d: UShort,
        val e: UShort,
        val f: UShort,
        val g: UShort,
        val h: UShort,
    ) : IpAddress()
}

/** Matches `golem:rdbms/types@1.5.0`'s `mac-address` record (6 bytes, align 1). */
data class MacAddress(val a: UByte, val b: UByte, val c: UByte, val d: UByte, val e: UByte, val f: UByte)

// ip-address: size=18 align=2, tag_size=1, payload_offset=2.
private fun liftIpAddress(base: Int): IpAddress {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + 2
    return when (tag) {
        0 -> IpAddress.Ipv4(loadByte(p).toUByte(), loadByte(p + 1).toUByte(), loadByte(p + 2).toUByte(), loadByte(p + 3).toUByte())
        1 -> IpAddress.Ipv6(
            loadShort(p).toUShort(),
            loadShort(p + 2).toUShort(),
            loadShort(p + 4).toUShort(),
            loadShort(p + 6).toUShort(),
            loadShort(p + 8).toUShort(),
            loadShort(p + 10).toUShort(),
            loadShort(p + 12).toUShort(),
            loadShort(p + 14).toUShort(),
        )
        else -> error("unknown ip-address tag $tag")
    }
}

private fun storeIpAddress(base: Int, ip: IpAddress) {
    val p = base + 2
    when (ip) {
        is IpAddress.Ipv4 -> {
            storeByte(base, 0)
            storeByte(p, ip.a.toByte())
            storeByte(p + 1, ip.b.toByte())
            storeByte(p + 2, ip.c.toByte())
            storeByte(p + 3, ip.d.toByte())
        }
        is IpAddress.Ipv6 -> {
            storeByte(base, 1)
            storeShort(p, ip.a.toShort())
            storeShort(p + 2, ip.b.toShort())
            storeShort(p + 4, ip.c.toShort())
            storeShort(p + 6, ip.d.toShort())
            storeShort(p + 8, ip.e.toShort())
            storeShort(p + 10, ip.f.toShort())
            storeShort(p + 12, ip.g.toShort())
            storeShort(p + 14, ip.h.toShort())
        }
    }
}

private fun liftMacAddress(base: Int): MacAddress = MacAddress(
    loadByte(base).toUByte(),
    loadByte(base + 1).toUByte(),
    loadByte(base + 2).toUByte(),
    loadByte(base + 3).toUByte(),
    loadByte(base + 4).toUByte(),
    loadByte(base + 5).toUByte(),
)

private fun storeMacAddress(base: Int, m: MacAddress) {
    storeByte(base, m.a.toByte())
    storeByte(base + 1, m.b.toByte())
    storeByte(base + 2, m.c.toByte())
    storeByte(base + 3, m.d.toByte())
    storeByte(base + 4, m.e.toByte())
    storeByte(base + 5, m.f.toByte())
}

internal fun liftListOfBool(base: Int): List<Boolean> {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> loadByte(ptr + i).toInt() != 0 }
}

internal fun lowerListOfBool(base: Int, bits: List<Boolean>) {
    val arr = alloc(bits.size, 1)
    bits.forEachIndexed { i, b -> storeByte(arr + i, if (b) 1 else 0) }
    storeInt(base, arr)
    storeInt(base + 4, bits.size)
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `int4bound` variant (8 bytes, align 4). */
sealed class Int4Bound {
    data class Included(val value: Int) : Int4Bound()
    data class Excluded(val value: Int) : Int4Bound()
    object Unbounded : Int4Bound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `int8bound` variant (16 bytes, align 8). */
sealed class Int8Bound {
    data class Included(val value: Long) : Int8Bound()
    data class Excluded(val value: Long) : Int8Bound()
    object Unbounded : Int8Bound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `numbound` variant (12 bytes, align 4). */
sealed class NumBound {
    data class Included(val value: String) : NumBound()
    data class Excluded(val value: String) : NumBound()
    object Unbounded : NumBound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `tsbound` variant (20 bytes, align 4). */
sealed class TsBound {
    data class Included(val value: DbTimestamp) : TsBound()
    data class Excluded(val value: DbTimestamp) : TsBound()
    object Unbounded : TsBound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `tstzbound` variant (24 bytes, align 4). */
sealed class TsTzBound {
    data class Included(val value: DbTimestampTz) : TsTzBound()
    data class Excluded(val value: DbTimestampTz) : TsTzBound()
    object Unbounded : TsTzBound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `datebound` variant (12 bytes, align 4). */
sealed class DateBound {
    data class Included(val value: DbDate) : DateBound()
    data class Excluded(val value: DbDate) : DateBound()
    object Unbounded : DateBound()
}

data class Int4Range(val start: Int4Bound, val end: Int4Bound)
data class Int8Range(val start: Int8Bound, val end: Int8Bound)
data class NumRange(val start: NumBound, val end: NumBound)
data class TsRange(val start: TsBound, val end: TsBound)
data class TsTzRange(val start: TsTzBound, val end: TsTzBound)
data class DateRange(val start: DateBound, val end: DateBound)

// All 6 bound variants: tag@0(1,1), payload@payload_offset. unbounded (tag=2) has no payload.
private fun liftInt4Bound(base: Int): Int4Bound = when (loadByte(base).toInt() and 0xFF) {
    0 -> Int4Bound.Included(loadInt(base + 4))
    1 -> Int4Bound.Excluded(loadInt(base + 4))
    else -> Int4Bound.Unbounded
}
private fun storeInt4Bound(base: Int, b: Int4Bound) {
    when (b) {
        is Int4Bound.Included -> {
            storeByte(base, 0)
            storeInt(base + 4, b.value)
        }
        is Int4Bound.Excluded -> {
            storeByte(base, 1)
            storeInt(base + 4, b.value)
        }
        Int4Bound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftInt8Bound(base: Int): Int8Bound = when (loadByte(base).toInt() and 0xFF) {
    0 -> Int8Bound.Included(loadLong(base + 8))
    1 -> Int8Bound.Excluded(loadLong(base + 8))
    else -> Int8Bound.Unbounded
}
private fun storeInt8Bound(base: Int, b: Int8Bound) {
    when (b) {
        is Int8Bound.Included -> {
            storeByte(base, 0)
            storeLong(base + 8, b.value)
        }
        is Int8Bound.Excluded -> {
            storeByte(base, 1)
            storeLong(base + 8, b.value)
        }
        Int8Bound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftNumBound(base: Int): NumBound = when (loadByte(base).toInt() and 0xFF) {
    0 -> NumBound.Included(liftString(loadInt(base + 4), loadInt(base + 8)))
    1 -> NumBound.Excluded(liftString(loadInt(base + 4), loadInt(base + 8)))
    else -> NumBound.Unbounded
}
private fun storeNumBound(base: Int, b: NumBound) {
    when (b) {
        is NumBound.Included -> {
            storeByte(base, 0)
            val (p, l) = lowerStringToPtrLen(b.value)
            storeInt(base + 4, p)
            storeInt(base + 8, l)
        }
        is NumBound.Excluded -> {
            storeByte(base, 1)
            val (p, l) = lowerStringToPtrLen(b.value)
            storeInt(base + 4, p)
            storeInt(base + 8, l)
        }
        NumBound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftTsBound(base: Int): TsBound = when (loadByte(base).toInt() and 0xFF) {
    0 -> TsBound.Included(liftDbTimestamp(base + 4))
    1 -> TsBound.Excluded(liftDbTimestamp(base + 4))
    else -> TsBound.Unbounded
}
private fun storeTsBound(base: Int, b: TsBound) {
    when (b) {
        is TsBound.Included -> {
            storeByte(base, 0)
            storeDbTimestamp(base + 4, b.value)
        }
        is TsBound.Excluded -> {
            storeByte(base, 1)
            storeDbTimestamp(base + 4, b.value)
        }
        TsBound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftTsTzBound(base: Int): TsTzBound = when (loadByte(base).toInt() and 0xFF) {
    0 -> TsTzBound.Included(liftDbTimestampTz(base + 4))
    1 -> TsTzBound.Excluded(liftDbTimestampTz(base + 4))
    else -> TsTzBound.Unbounded
}
private fun storeTsTzBound(base: Int, b: TsTzBound) {
    when (b) {
        is TsTzBound.Included -> {
            storeByte(base, 0)
            storeDbTimestampTz(base + 4, b.value)
        }
        is TsTzBound.Excluded -> {
            storeByte(base, 1)
            storeDbTimestampTz(base + 4, b.value)
        }
        TsTzBound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftDateBound(base: Int): DateBound = when (loadByte(base).toInt() and 0xFF) {
    0 -> DateBound.Included(liftDbDate(base + 4))
    1 -> DateBound.Excluded(liftDbDate(base + 4))
    else -> DateBound.Unbounded
}
private fun storeDateBound(base: Int, b: DateBound) {
    when (b) {
        is DateBound.Included -> {
            storeByte(base, 0)
            storeDbDate(base + 4, b.value)
        }
        is DateBound.Excluded -> {
            storeByte(base, 1)
            storeDbDate(base + 4, b.value)
        }
        DateBound.Unbounded -> storeByte(base, 2)
    }
}

// Ranges: {start: Bound @0, end: Bound @<start's own size>}.
private fun liftInt4Range(base: Int): Int4Range = Int4Range(liftInt4Bound(base), liftInt4Bound(base + 8))
private fun storeInt4Range(base: Int, r: Int4Range) {
    storeInt4Bound(base, r.start)
    storeInt4Bound(base + 8, r.end)
}
private fun liftInt8Range(base: Int): Int8Range = Int8Range(liftInt8Bound(base), liftInt8Bound(base + 16))
private fun storeInt8Range(base: Int, r: Int8Range) {
    storeInt8Bound(base, r.start)
    storeInt8Bound(base + 16, r.end)
}
private fun liftNumRange(base: Int): NumRange = NumRange(liftNumBound(base), liftNumBound(base + 12))
private fun storeNumRange(base: Int, r: NumRange) {
    storeNumBound(base, r.start)
    storeNumBound(base + 12, r.end)
}
private fun liftTsRange(base: Int): TsRange = TsRange(liftTsBound(base), liftTsBound(base + 20))
private fun storeTsRange(base: Int, r: TsRange) {
    storeTsBound(base, r.start)
    storeTsBound(base + 20, r.end)
}
private fun liftTsTzRange(base: Int): TsTzRange = TsTzRange(liftTsTzBound(base), liftTsTzBound(base + 24))
private fun storeTsTzRange(base: Int, r: TsTzRange) {
    storeTsTzBound(base, r.start)
    storeTsTzBound(base + 24, r.end)
}
private fun liftDateRange(base: Int): DateRange = DateRange(liftDateBound(base), liftDateBound(base + 12))
private fun storeDateRange(base: Int, r: DateRange) {
    storeDateBound(base, r.start)
    storeDateBound(base + 12, r.end)
}

private fun liftListOfFloat(base: Int): List<Float> {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> loadFloat(ptr + i * 4) }
}
private fun lowerListOfFloat(base: Int, values: List<Float>) {
    val arr = alloc(values.size * 4, 4)
    values.forEachIndexed { i, v -> storeFloat(arr + i * 4, v) }
    storeInt(base, arr)
    storeInt(base + 4, values.size)
}
private fun liftListOfInt(base: Int): List<Int> {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> loadInt(ptr + i * 4) }
}
private fun lowerListOfInt(base: Int, values: List<Int>) {
    val arr = alloc(values.size * 4, 4)
    values.forEachIndexed { i, v -> storeInt(arr + i * 4, v) }
    storeInt(base, arr)
    storeInt(base + 4, values.size)
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `sparse-vec` record (20 bytes, align 4). */
data class SparseVec(val dim: Int, val indices: List<Int>, val values: List<Float>)

// sparse-vec: dim@0(4,4), indices@4(list<s32>,8,4), values@12(list<f32>,8,4).
private fun liftSparseVec(base: Int): SparseVec = SparseVec(loadInt(base), liftListOfInt(base + 4), liftListOfFloat(base + 12))
private fun storeSparseVec(base: Int, v: SparseVec) {
    storeInt(base, v.dim)
    lowerListOfInt(base + 4, v.indices)
    lowerListOfFloat(base + 12, v.values)
}

// Resolves an owned lazy-db-value handle to its PostgresDbValue and drops the handle in the
// same step. Every consumer of a nested value (composite.values, domain.value, array elements,
// value-bound's included/excluded) wants the fully-materialized Kotlin value, not a live
// handle -- matching this SDK's established style of fully-materialized value trees (see
// SchemaValue.kt) rather than exposing resource handles to callers for recursive data. This is
// a deliberate ergonomic choice: real Postgres composite/array/domain/range nesting is finite,
// so eager recursive resolution is the right default (a lazy, handle-exposing API would only
// matter for pathologically deep or wide values, not realistic schemas).
private fun liftAndConsumeLazyDbValue(handle: Int): PostgresDbValue {
    val retPtr = alloc(DBV_SIZE, DBV_ALIGN)
    hostLazyDbValueGet(handle, retPtr)
    val v = liftDbValue(retPtr)
    hostLazyDbValueDrop(handle)
    return v
}

private fun lowerToLazyDbValueHandle(value: PostgresDbValue): Int {
    val base = alloc(DBV_SIZE, DBV_ALIGN)
    lowerDbValue(base, value)
    return hostLazyDbValueConstructor(base)
}

private fun liftListOfLazyDbValue(base: Int): List<PostgresDbValue> {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> liftAndConsumeLazyDbValue(loadInt(ptr + i * 4)) }
}
private fun lowerListOfLazyDbValue(base: Int, values: List<PostgresDbValue>) {
    val arr = alloc(values.size * 4, 4)
    values.forEachIndexed { i, v -> storeInt(arr + i * 4, lowerToLazyDbValueHandle(v)) }
    storeInt(base, arr)
    storeInt(base + 4, values.size)
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `enumeration` record (16 bytes, align 4) -- flat, no `lazy-db-value` involved. */
data class PostgresEnumeration(val name: String, val value: String)

/** Matches `golem:rdbms/postgres@1.5.0`'s `composite` record (16 bytes, align 4). `values` is fully materialized (see [liftAndConsumeLazyDbValue]). */
data class PostgresComposite(val name: String, val values: List<PostgresDbValue>)

/** Matches `golem:rdbms/postgres@1.5.0`'s `domain` record (12 bytes, align 4). `value` is fully materialized. */
data class PostgresDomain(val name: String, val value: PostgresDbValue)

/** Matches `golem:rdbms/postgres@1.5.0`'s `value-bound` variant (8 bytes, align 4). Payloads are fully materialized. */
sealed class ValueBound {
    data class Included(val value: PostgresDbValue) : ValueBound()
    data class Excluded(val value: PostgresDbValue) : ValueBound()
    object Unbounded : ValueBound()
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `values-range` record (16 bytes, align 4). */
data class ValuesRange(val start: ValueBound, val end: ValueBound)

/** Matches `golem:rdbms/postgres@1.5.0`'s `range` record (24 bytes, align 4). */
data class PostgresRange(val name: String, val value: ValuesRange)

private fun liftEnumeration(base: Int): PostgresEnumeration = PostgresEnumeration(liftString(loadInt(base), loadInt(base + 4)), liftString(loadInt(base + 8), loadInt(base + 12)))
private fun storeEnumeration(base: Int, e: PostgresEnumeration) {
    val (np, nl) = lowerStringToPtrLen(e.name)
    storeInt(base, np)
    storeInt(base + 4, nl)
    val (vp, vl) = lowerStringToPtrLen(e.value)
    storeInt(base + 8, vp)
    storeInt(base + 12, vl)
}

// composite: name@0(8,4), values@8(list<lazy-db-value>,8,4).
private fun liftComposite(base: Int): PostgresComposite {
    val name = liftString(loadInt(base), loadInt(base + 4))
    return PostgresComposite(name, liftListOfLazyDbValue(base + 8))
}
private fun storeComposite(base: Int, c: PostgresComposite) {
    val (np, nl) = lowerStringToPtrLen(c.name)
    storeInt(base, np)
    storeInt(base + 4, nl)
    lowerListOfLazyDbValue(base + 8, c.values)
}

// domain: name@0(8,4), value@8(own<lazy-db-value>: i32 handle, 4,4).
private fun liftDomain(base: Int): PostgresDomain {
    val name = liftString(loadInt(base), loadInt(base + 4))
    return PostgresDomain(name, liftAndConsumeLazyDbValue(loadInt(base + 8)))
}
private fun storeDomain(base: Int, d: PostgresDomain) {
    val (np, nl) = lowerStringToPtrLen(d.name)
    storeInt(base, np)
    storeInt(base + 4, nl)
    storeInt(base + 8, lowerToLazyDbValueHandle(d.value))
}

// value-bound: tag@0(1,1), payload@4 (own<lazy-db-value>: i32 handle, only for included/excluded).
private fun liftValueBound(base: Int): ValueBound = when (loadByte(base).toInt() and 0xFF) {
    0 -> ValueBound.Included(liftAndConsumeLazyDbValue(loadInt(base + 4)))
    1 -> ValueBound.Excluded(liftAndConsumeLazyDbValue(loadInt(base + 4)))
    else -> ValueBound.Unbounded
}
private fun storeValueBound(base: Int, b: ValueBound) {
    when (b) {
        is ValueBound.Included -> {
            storeByte(base, 0)
            storeInt(base + 4, lowerToLazyDbValueHandle(b.value))
        }
        is ValueBound.Excluded -> {
            storeByte(base, 1)
            storeInt(base + 4, lowerToLazyDbValueHandle(b.value))
        }
        ValueBound.Unbounded -> storeByte(base, 2)
    }
}

private fun liftValuesRange(base: Int): ValuesRange = ValuesRange(liftValueBound(base), liftValueBound(base + 8))
private fun storeValuesRange(base: Int, r: ValuesRange) {
    storeValueBound(base, r.start)
    storeValueBound(base + 8, r.end)
}

// range: name@0(8,4), value@8(values-range,16,4).
private fun liftRange(base: Int): PostgresRange {
    val name = liftString(loadInt(base), loadInt(base + 4))
    return PostgresRange(name, liftValuesRange(base + 8))
}
private fun storeRange(base: Int, r: PostgresRange) {
    val (np, nl) = lowerStringToPtrLen(r.name)
    storeInt(base, np)
    storeInt(base + 4, nl)
    storeValuesRange(base + 8, r.value)
}

/**
 * Matches `golem:rdbms/postgres@1.5.0`'s `db-value` variant IN FULL (45 cases, tags 0-44).
 * `enumeration`/`composite`/`domain`/`array`/`range` (tags 36-40) go through the
 * `lazy-db-value` resource (`constructor(value: db-value); get() -> db-value;`) for their
 * nested values, but are fully materialized into plain Kotlin data here -- see
 * [liftAndConsumeLazyDbValue]'s doc comment for why. `query-stream`/`db-result-stream` (lazy,
 * paginated results) remain a separate deferred piece -- a different resource, for a different
 * purpose (streaming a `db-result` incrementally, not a single recursive value).
 */
sealed class PostgresDbValue {
    data class Character(val value: Byte) : PostgresDbValue()
    data class Int2(val value: Short) : PostgresDbValue()
    data class Int4(val value: Int) : PostgresDbValue()
    data class Int8(val value: Long) : PostgresDbValue()
    data class Float4(val value: Float) : PostgresDbValue()
    data class Float8(val value: Double) : PostgresDbValue()

    /** WIT represents `numeric` as an arbitrary-precision decimal encoded as a string. */
    data class Numeric(val value: String) : PostgresDbValue()
    data class BooleanVal(val value: Boolean) : PostgresDbValue()
    data class Text(val value: String) : PostgresDbValue()
    data class Varchar(val value: String) : PostgresDbValue()
    data class Bpchar(val value: String) : PostgresDbValue()
    data class TimestampVal(val value: DbTimestamp) : PostgresDbValue()
    data class TimestampTzVal(val value: DbTimestampTz) : PostgresDbValue()
    data class DateVal(val value: DbDate) : PostgresDbValue()
    data class TimeVal(val value: DbTime) : PostgresDbValue()
    data class TimeTzVal(val value: DbTimeTz) : PostgresDbValue()
    data class IntervalVal(val value: DbInterval) : PostgresDbValue()
    data class Bytea(val value: List<UByte>) : PostgresDbValue()
    data class Json(val value: String) : PostgresDbValue()
    data class Jsonb(val value: String) : PostgresDbValue()
    data class JsonPath(val value: String) : PostgresDbValue()
    data class Xml(val value: String) : PostgresDbValue()
    data class Uuid(val highBits: Long, val lowBits: Long) : PostgresDbValue()
    data class InetVal(val value: IpAddress) : PostgresDbValue()
    data class CidrVal(val value: IpAddress) : PostgresDbValue()
    data class MacaddrVal(val value: MacAddress) : PostgresDbValue()
    data class Bit(val value: List<Boolean>) : PostgresDbValue()
    data class Varbit(val value: List<Boolean>) : PostgresDbValue()
    data class Int4RangeVal(val value: Int4Range) : PostgresDbValue()
    data class Int8RangeVal(val value: Int8Range) : PostgresDbValue()
    data class NumRangeVal(val value: NumRange) : PostgresDbValue()
    data class TsRangeVal(val value: TsRange) : PostgresDbValue()
    data class TsTzRangeVal(val value: TsTzRange) : PostgresDbValue()
    data class DateRangeVal(val value: DateRange) : PostgresDbValue()
    data class Money(val value: Long) : PostgresDbValue()
    data class Oid(val value: UInt) : PostgresDbValue()
    data class EnumerationVal(val value: PostgresEnumeration) : PostgresDbValue()
    data class CompositeVal(val value: PostgresComposite) : PostgresDbValue()
    data class DomainVal(val value: PostgresDomain) : PostgresDbValue()
    data class ArrayVal(val value: List<PostgresDbValue>) : PostgresDbValue()
    data class RangeVal(val value: PostgresRange) : PostgresDbValue()
    object Null : PostgresDbValue()
    data class VectorVal(val value: List<Float>) : PostgresDbValue()
    data class HalfvecVal(val value: List<Float>) : PostgresDbValue()
    data class SparsevecVal(val value: SparseVec) : PostgresDbValue()
}

// db-value: size=56 align=8, tag_size=1, payload_offset=8. Tag numbers are this variant's
// absolute case indices (0-44), verified via abi-dump against the full 45-case variant.
private const val DBV_SIZE = 56
private const val DBV_ALIGN = 8
private const val DBV_PAYLOAD_OFFSET = 8

private fun liftDbValue(base: Int): PostgresDbValue {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + DBV_PAYLOAD_OFFSET
    return when (tag) {
        0 -> PostgresDbValue.Character(loadByte(p))
        1 -> PostgresDbValue.Int2(loadShort(p))
        2 -> PostgresDbValue.Int4(loadInt(p))
        3 -> PostgresDbValue.Int8(loadLong(p))
        4 -> PostgresDbValue.Float4(loadFloat(p))
        5 -> PostgresDbValue.Float8(loadDouble(p))
        6 -> PostgresDbValue.Numeric(liftString(loadInt(p), loadInt(p + 4)))
        7 -> PostgresDbValue.BooleanVal(loadByte(p).toInt() != 0)
        8 -> PostgresDbValue.Text(liftString(loadInt(p), loadInt(p + 4)))
        9 -> PostgresDbValue.Varchar(liftString(loadInt(p), loadInt(p + 4)))
        10 -> PostgresDbValue.Bpchar(liftString(loadInt(p), loadInt(p + 4)))
        11 -> PostgresDbValue.TimestampVal(liftDbTimestamp(p))
        12 -> PostgresDbValue.TimestampTzVal(liftDbTimestampTz(p))
        13 -> PostgresDbValue.DateVal(liftDbDate(p))
        14 -> PostgresDbValue.TimeVal(liftDbTime(p))
        15 -> PostgresDbValue.TimeTzVal(liftDbTimeTz(p))
        16 -> PostgresDbValue.IntervalVal(liftDbInterval(p))
        17 -> {
            val ptr = loadInt(p)
            val len = loadInt(p + 4)
            PostgresDbValue.Bytea((0 until len).map { i -> loadByte(ptr + i).toUByte() })
        }
        18 -> PostgresDbValue.Json(liftString(loadInt(p), loadInt(p + 4)))
        19 -> PostgresDbValue.Jsonb(liftString(loadInt(p), loadInt(p + 4)))
        20 -> PostgresDbValue.JsonPath(liftString(loadInt(p), loadInt(p + 4)))
        21 -> PostgresDbValue.Xml(liftString(loadInt(p), loadInt(p + 4)))
        22 -> PostgresDbValue.Uuid(loadLong(p), loadLong(p + 8))
        23 -> PostgresDbValue.InetVal(liftIpAddress(p))
        24 -> PostgresDbValue.CidrVal(liftIpAddress(p))
        25 -> PostgresDbValue.MacaddrVal(liftMacAddress(p))
        26 -> PostgresDbValue.Bit(liftListOfBool(p))
        27 -> PostgresDbValue.Varbit(liftListOfBool(p))
        28 -> PostgresDbValue.Int4RangeVal(liftInt4Range(p))
        29 -> PostgresDbValue.Int8RangeVal(liftInt8Range(p))
        30 -> PostgresDbValue.NumRangeVal(liftNumRange(p))
        31 -> PostgresDbValue.TsRangeVal(liftTsRange(p))
        32 -> PostgresDbValue.TsTzRangeVal(liftTsTzRange(p))
        33 -> PostgresDbValue.DateRangeVal(liftDateRange(p))
        34 -> PostgresDbValue.Money(loadLong(p))
        35 -> PostgresDbValue.Oid(loadInt(p).toUInt())
        36 -> PostgresDbValue.EnumerationVal(liftEnumeration(p))
        37 -> PostgresDbValue.CompositeVal(liftComposite(p))
        38 -> PostgresDbValue.DomainVal(liftDomain(p))
        39 -> PostgresDbValue.ArrayVal(liftListOfLazyDbValue(p))
        40 -> PostgresDbValue.RangeVal(liftRange(p))
        41 -> PostgresDbValue.Null
        42 -> PostgresDbValue.VectorVal(liftListOfFloat(p))
        43 -> PostgresDbValue.HalfvecVal(liftListOfFloat(p))
        44 -> PostgresDbValue.SparsevecVal(liftSparseVec(p))
        else -> error("native Rdbms: unknown db-value tag $tag")
    }
}

private fun lowerDbValue(base: Int, value: PostgresDbValue) {
    val p = base + DBV_PAYLOAD_OFFSET
    when (value) {
        is PostgresDbValue.Character -> {
            storeByte(base, 0)
            storeByte(p, value.value)
        }
        is PostgresDbValue.Int2 -> {
            storeByte(base, 1)
            storeShort(p, value.value)
        }
        is PostgresDbValue.Int4 -> {
            storeByte(base, 2)
            storeInt(p, value.value)
        }
        is PostgresDbValue.Int8 -> {
            storeByte(base, 3)
            storeLong(p, value.value)
        }
        is PostgresDbValue.Float4 -> {
            storeByte(base, 4)
            storeFloat(p, value.value)
        }
        is PostgresDbValue.Float8 -> {
            storeByte(base, 5)
            storeDouble(p, value.value)
        }
        is PostgresDbValue.Numeric -> {
            storeByte(base, 6)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.BooleanVal -> {
            storeByte(base, 7)
            storeByte(p, if (value.value) 1 else 0)
        }
        is PostgresDbValue.Text -> {
            storeByte(base, 8)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.Varchar -> {
            storeByte(base, 9)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.Bpchar -> {
            storeByte(base, 10)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.TimestampVal -> {
            storeByte(base, 11)
            storeDbTimestamp(p, value.value)
        }
        is PostgresDbValue.TimestampTzVal -> {
            storeByte(base, 12)
            storeDbTimestampTz(p, value.value)
        }
        is PostgresDbValue.DateVal -> {
            storeByte(base, 13)
            storeDbDate(p, value.value)
        }
        is PostgresDbValue.TimeVal -> {
            storeByte(base, 14)
            storeDbTime(p, value.value)
        }
        is PostgresDbValue.TimeTzVal -> {
            storeByte(base, 15)
            storeDbTimeTz(p, value.value)
        }
        is PostgresDbValue.IntervalVal -> {
            storeByte(base, 16)
            storeDbInterval(p, value.value)
        }
        is PostgresDbValue.Bytea -> {
            storeByte(base, 17)
            val arr = alloc(value.value.size, 1)
            value.value.forEachIndexed { i, b -> storeByte(arr + i, b.toByte()) }
            storeInt(p, arr)
            storeInt(p + 4, value.value.size)
        }
        is PostgresDbValue.Json -> {
            storeByte(base, 18)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.Jsonb -> {
            storeByte(base, 19)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.JsonPath -> {
            storeByte(base, 20)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.Xml -> {
            storeByte(base, 21)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is PostgresDbValue.Uuid -> {
            storeByte(base, 22)
            storeLong(p, value.highBits)
            storeLong(p + 8, value.lowBits)
        }
        is PostgresDbValue.InetVal -> {
            storeByte(base, 23)
            storeIpAddress(p, value.value)
        }
        is PostgresDbValue.CidrVal -> {
            storeByte(base, 24)
            storeIpAddress(p, value.value)
        }
        is PostgresDbValue.MacaddrVal -> {
            storeByte(base, 25)
            storeMacAddress(p, value.value)
        }
        is PostgresDbValue.Bit -> {
            storeByte(base, 26)
            lowerListOfBool(p, value.value)
        }
        is PostgresDbValue.Varbit -> {
            storeByte(base, 27)
            lowerListOfBool(p, value.value)
        }
        is PostgresDbValue.Int4RangeVal -> {
            storeByte(base, 28)
            storeInt4Range(p, value.value)
        }
        is PostgresDbValue.Int8RangeVal -> {
            storeByte(base, 29)
            storeInt8Range(p, value.value)
        }
        is PostgresDbValue.NumRangeVal -> {
            storeByte(base, 30)
            storeNumRange(p, value.value)
        }
        is PostgresDbValue.TsRangeVal -> {
            storeByte(base, 31)
            storeTsRange(p, value.value)
        }
        is PostgresDbValue.TsTzRangeVal -> {
            storeByte(base, 32)
            storeTsTzRange(p, value.value)
        }
        is PostgresDbValue.DateRangeVal -> {
            storeByte(base, 33)
            storeDateRange(p, value.value)
        }
        is PostgresDbValue.Money -> {
            storeByte(base, 34)
            storeLong(p, value.value)
        }
        is PostgresDbValue.Oid -> {
            storeByte(base, 35)
            storeInt(p, value.value.toInt())
        }
        is PostgresDbValue.EnumerationVal -> {
            storeByte(base, 36)
            storeEnumeration(p, value.value)
        }
        is PostgresDbValue.CompositeVal -> {
            storeByte(base, 37)
            storeComposite(p, value.value)
        }
        is PostgresDbValue.DomainVal -> {
            storeByte(base, 38)
            storeDomain(p, value.value)
        }
        is PostgresDbValue.ArrayVal -> {
            storeByte(base, 39)
            lowerListOfLazyDbValue(p, value.value)
        }
        is PostgresDbValue.RangeVal -> {
            storeByte(base, 40)
            storeRange(p, value.value)
        }
        PostgresDbValue.Null -> storeByte(base, 41)
        is PostgresDbValue.VectorVal -> {
            storeByte(base, 42)
            lowerListOfFloat(p, value.value)
        }
        is PostgresDbValue.HalfvecVal -> {
            storeByte(base, 43)
            lowerListOfFloat(p, value.value)
        }
        is PostgresDbValue.SparsevecVal -> {
            storeByte(base, 44)
            storeSparseVec(p, value.value)
        }
    }
}

private fun lowerDbValueList(values: List<PostgresDbValue>): Pair<Int, Int> {
    val arr = alloc(values.size * DBV_SIZE, DBV_ALIGN)
    values.forEachIndexed { i, v -> lowerDbValue(arr + i * DBV_SIZE, v) }
    return arr to values.size
}

/** Mirrors Scala's own `DbColumn`: deliberately omits `db-type` (the 40-case `db-column-type`
 * variant describing the column's SQL type structurally) -- Scala's reference `DbColumn` case
 * class only carries `dbTypeName`, not the structural type, so this SDK matches that scope
 * rather than decoding a type variant nothing consumes. */
data class DbColumn(val ordinal: Long, val name: String, val dbTypeName: String)

/**
 * A textual representation of [this] value for [PostgresDbRow]'s convenience accessors.
 * `Text`/`Varchar`/`Bpchar`/`Numeric`/`Json`/`Jsonb`/`JsonPath`/`Xml` return their raw string
 * content directly; simple numeric/boolean cases return their number/boolean's own
 * `toString()`; everything else (temporal, network, bit, range, composite/domain/array/range,
 * vector types) falls back to Kotlin's data-class `toString()`, which is at least an accurate
 * structural dump.
 *
 * NOTE: Scala's reference `PostgresDbRow.getString`/`getInt`/`getLong` fall back to `v.toString`
 * with no `PostgresDbValue` `toString` override, so for anything other than the numeric fast
 * paths it returns Scala's auto-generated case-class dump (e.g. `Text("hello").toString ==
 * "Text(hello)"`, not `"hello"`) -- almost certainly an unintentional bug in the reference, not
 * a deliberate scope choice (a `getString` that can't extract a text column's actual text isn't
 * a usable accessor). This port fixes that by extracting the real value for the common cases
 * instead of reproducing the bug; the accessors' overall shape (Null -> null, Int4/Int2 fast
 * path for `getInt`, Int8/Int4 fast path for `getLong`) still matches Scala's intent exactly.
 */
private fun PostgresDbValue.asDisplayString(): String = when (this) {
    is PostgresDbValue.Character -> value.toString()
    is PostgresDbValue.Int2 -> value.toString()
    is PostgresDbValue.Int4 -> value.toString()
    is PostgresDbValue.Int8 -> value.toString()
    is PostgresDbValue.Float4 -> value.toString()
    is PostgresDbValue.Float8 -> value.toString()
    is PostgresDbValue.Numeric -> value
    is PostgresDbValue.BooleanVal -> value.toString()
    is PostgresDbValue.Text -> value
    is PostgresDbValue.Varchar -> value
    is PostgresDbValue.Bpchar -> value
    is PostgresDbValue.Json -> value
    is PostgresDbValue.Jsonb -> value
    is PostgresDbValue.JsonPath -> value
    is PostgresDbValue.Xml -> value
    is PostgresDbValue.Money -> value.toString()
    is PostgresDbValue.Oid -> value.toString()
    else -> toString()
}

data class PostgresDbRow(val values: List<PostgresDbValue>) {
    fun getString(index: Int): String? = when (val v = values[index]) {
        PostgresDbValue.Null -> null
        else -> v.asDisplayString()
    }

    fun getInt(index: Int): Int? = when (val v = values[index]) {
        PostgresDbValue.Null -> null
        is PostgresDbValue.Int4 -> v.value
        is PostgresDbValue.Int2 -> v.value.toInt()
        else -> v.asDisplayString().toInt()
    }

    fun getLong(index: Int): Long? = when (val v = values[index]) {
        PostgresDbValue.Null -> null
        is PostgresDbValue.Int8 -> v.value
        is PostgresDbValue.Int4 -> v.value.toLong()
        else -> v.asDisplayString().toLong()
    }
}

data class PostgresDbResult(val columns: List<DbColumn>, val rows: List<PostgresDbRow>)

// db-column: size=48 align=8 { ordinal@0(8,8), name@8(8,4), db-type@16(20,4, skipped), db-type-name@36(8,4) }.
private fun liftDbColumn(base: Int): DbColumn = DbColumn(
    ordinal = loadLong(base),
    name = liftString(loadInt(base + 8), loadInt(base + 12)),
    dbTypeName = liftString(loadInt(base + 36), loadInt(base + 40)),
)

// db-row: size=8 align=4 { values: list<db-value> @0 }.
private fun liftDbRow(base: Int): PostgresDbRow {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return PostgresDbRow((0 until len).map { i -> liftDbValue(ptr + i * DBV_SIZE) })
}

// db-result: size=16 align=4 { columns: list<db-column> @0, rows: list<db-row> @8 }.
private fun liftDbResult(base: Int): PostgresDbResult {
    val colPtr = loadInt(base)
    val colLen = loadInt(base + 4)
    val rowPtr = loadInt(base + 8)
    val rowLen = loadInt(base + 12)
    return PostgresDbResult(
        columns = (0 until colLen).map { i -> liftDbColumn(colPtr + i * 48) },
        rows = (0 until rowLen).map { i -> liftDbRow(rowPtr + i * 8) },
    )
}

/** Matches `golem:rdbms/postgres@1.5.0`'s `error` variant (6 cases, all string payloads) -- same shape as Scala's `DbError`. */
sealed class DbError {
    data class ConnectionFailure(val message: String) : DbError()
    data class QueryParameterFailure(val message: String) : DbError()
    data class QueryExecutionFailure(val message: String) : DbError()
    data class QueryResponseFailure(val message: String) : DbError()
    data class Other(val message: String) : DbError()

    /** The resident transaction or result stream cannot be resumed after a restart; recorded results stay valid. Added in Golem 1.6.0. */
    data class ResourceLost(val message: String) : DbError()
}

// error: size=12 align=4, tag_size=1, payload_offset=4 (all 5 cases: string, 8 bytes).
private fun liftDbError(base: Int): DbError {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + 4
    val msg = liftString(loadInt(p), loadInt(p + 4))
    return when (tag) {
        0 -> DbError.ConnectionFailure(msg)
        1 -> DbError.QueryParameterFailure(msg)
        2 -> DbError.QueryExecutionFailure(msg)
        3 -> DbError.QueryResponseFailure(msg)
        4 -> DbError.Other(msg)
        5 -> DbError.ResourceLost(msg)
        else -> error("unknown golem:rdbms error tag $tag")
    }
}

/** A live connection to a Postgres database. MUST be [close]d when done. */
class PostgresConnection internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, PostgresDbResult> {
        check(!closed) { "PostgresConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerDbValueList(params)
        val retPtr = alloc(20, 4) // result<db-result, error>: tag@0(1,1), payload@4(max(16,12)=16)
        hostConnectionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftDbResult(retPtr + 4)) else Either.Left(liftDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, Long> {
        check(!closed) { "PostgresConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerDbValueList(params)
        val retPtr = alloc(24, 8) // result<u64, error>: tag@0(1,1), payload@8 (rounded to align8) -- max(8,12)=12 -> 8+12=20 -> round to 24
        hostConnectionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftDbError(retPtr + 8))
    }

    fun beginTransaction(): Either<DbError, PostgresTransaction> {
        check(!closed) { "PostgresConnection already closed" }
        val retPtr = alloc(16, 4) // result<db-transaction, error>: tag@0(1,1), payload@4(max(4,12)=12)
        hostConnectionBeginTransaction(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(PostgresTransaction(loadInt(retPtr + 4))) else Either.Left(liftDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostConnectionDrop(handle)
            closed = true
        }
    }

    companion object {
        fun open(address: String): Either<DbError, PostgresConnection> {
            val (ptr, len) = lowerStringToPtrLen(address)
            val retPtr = alloc(16, 4) // result<db-connection, error>: tag@0(1,1), payload@4(max(4,12)=12)
            hostConnectionOpen(ptr, len, retPtr)
            return if (loadByte(retPtr).toInt() == 0) Either.Right(PostgresConnection(loadInt(retPtr + 4))) else Either.Left(liftDbError(retPtr + 4))
        }
    }
}

/** An open transaction on a [PostgresConnection]. MUST be [close]d when done, whether or not [commit]/[rollback] was called. */
class PostgresTransaction internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, PostgresDbResult> {
        check(!closed) { "PostgresTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerDbValueList(params)
        val retPtr = alloc(20, 4)
        hostTransactionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftDbResult(retPtr + 4)) else Either.Left(liftDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, Long> {
        check(!closed) { "PostgresTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerDbValueList(params)
        val retPtr = alloc(24, 8)
        hostTransactionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftDbError(retPtr + 8))
    }

    fun commit(): Either<DbError, Unit> {
        check(!closed) { "PostgresTransaction already closed" }
        val retPtr = alloc(16, 4) // result<_, error>: tag@0(1,1), payload@4(12)
        hostTransactionCommit(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftDbError(retPtr + 4))
    }

    fun rollback(): Either<DbError, Unit> {
        check(!closed) { "PostgresTransaction already closed" }
        val retPtr = alloc(16, 4)
        hostTransactionRollback(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostTransactionDrop(handle)
            closed = true
        }
    }
}
