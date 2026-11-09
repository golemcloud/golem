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

// ===========================================================================
// Ignite (golem:rdbms/ignite2@1.5.0)
// ===========================================================================
//
// Entirely flat, like MySQL -- no lazy-db-value-style recursion. Smallest of the three
// backends (16 db-value cases including null, vs Postgres's 45 and MySQL's 36). db-column is
// simpler too: just {ordinal, name}, no db-type-name at all (matching Scala's own
// IgniteDbColumn, which likewise has no type-name field). execute returns s64 here, not u64
// like Postgres/MySQL. Signatures verified via abi-dump.

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[static]db-connection.open")
private external fun hostIgniteConnectionOpen(addrPtr: Int, addrLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-connection.query")
private external fun hostIgniteConnectionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-connection.execute")
private external fun hostIgniteConnectionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-connection.begin-transaction")
private external fun hostIgniteConnectionBeginTransaction(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[resource-drop]db-connection")
private external fun hostIgniteConnectionDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-transaction.query")
private external fun hostIgniteTransactionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-transaction.execute")
private external fun hostIgniteTransactionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-transaction.commit")
private external fun hostIgniteTransactionCommit(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[method]db-transaction.rollback")
private external fun hostIgniteTransactionRollback(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/ignite2@1.5.0", "[resource-drop]db-transaction")
private external fun hostIgniteTransactionDrop(handle: Int)

/** Matches `golem:rdbms/ignite2@1.5.0`'s `db-value` variant in full (16 cases, tags 0-15). */
sealed class IgniteDbValue {
    object DbNull : IgniteDbValue()
    data class DbBoolean(val value: Boolean) : IgniteDbValue()
    data class DbByte(val value: Byte) : IgniteDbValue()
    data class DbShort(val value: Short) : IgniteDbValue()
    data class DbInt(val value: Int) : IgniteDbValue()
    data class DbLong(val value: Long) : IgniteDbValue()
    data class DbFloat(val value: Float) : IgniteDbValue()
    data class DbDouble(val value: Double) : IgniteDbValue()

    /** A 16-bit Unicode code unit (Java `char`). */
    data class DbChar(val value: Char) : IgniteDbValue()
    data class DbString(val value: String) : IgniteDbValue()
    data class DbUuid(val highBits: Long, val lowBits: Long) : IgniteDbValue()

    /** Milliseconds since Unix epoch (UTC). */
    data class DbDate(val millis: Long) : IgniteDbValue()

    /** Milliseconds since epoch, plus sub-millisecond nanoseconds (0..999_999). */
    data class DbTimestamp(val millis: Long, val nanos: Int) : IgniteDbValue()

    /** Nanoseconds since midnight. */
    data class DbTime(val nanos: Long) : IgniteDbValue()
    data class DbDecimal(val value: String) : IgniteDbValue()
    data class DbByteArray(val value: List<UByte>) : IgniteDbValue()
}

// db-value: size=24 align=8, tag_size=1, payload_offset=8.
private const val IGNITE_DBV_SIZE = 24
private const val IGNITE_DBV_ALIGN = 8
private const val IGNITE_DBV_PAYLOAD_OFFSET = 8

private fun liftIgniteDbValue(base: Int): IgniteDbValue {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + IGNITE_DBV_PAYLOAD_OFFSET
    return when (tag) {
        0 -> IgniteDbValue.DbNull
        1 -> IgniteDbValue.DbBoolean(loadByte(p).toInt() != 0)
        2 -> IgniteDbValue.DbByte(loadByte(p))
        3 -> IgniteDbValue.DbShort(loadShort(p))
        4 -> IgniteDbValue.DbInt(loadInt(p))
        5 -> IgniteDbValue.DbLong(loadLong(p))
        6 -> IgniteDbValue.DbFloat(loadFloat(p))
        7 -> IgniteDbValue.DbDouble(loadDouble(p))
        8 -> IgniteDbValue.DbChar((loadShort(p).toInt() and 0xFFFF).toChar())
        9 -> IgniteDbValue.DbString(liftString(loadInt(p), loadInt(p + 4)))
        10 -> IgniteDbValue.DbUuid(loadLong(p), loadLong(p + 8))
        11 -> IgniteDbValue.DbDate(loadLong(p))
        12 -> IgniteDbValue.DbTimestamp(loadLong(p), loadInt(p + 8))
        13 -> IgniteDbValue.DbTime(loadLong(p))
        14 -> IgniteDbValue.DbDecimal(liftString(loadInt(p), loadInt(p + 4)))
        15 -> IgniteDbValue.DbByteArray(liftListOfUByte(p))
        else -> error("native Rdbms: unknown ignite db-value tag $tag")
    }
}

private fun lowerIgniteDbValue(base: Int, value: IgniteDbValue) {
    val p = base + IGNITE_DBV_PAYLOAD_OFFSET
    when (value) {
        IgniteDbValue.DbNull -> storeByte(base, 0)
        is IgniteDbValue.DbBoolean -> {
            storeByte(base, 1)
            storeByte(p, if (value.value) 1 else 0)
        }
        is IgniteDbValue.DbByte -> {
            storeByte(base, 2)
            storeByte(p, value.value)
        }
        is IgniteDbValue.DbShort -> {
            storeByte(base, 3)
            storeShort(p, value.value)
        }
        is IgniteDbValue.DbInt -> {
            storeByte(base, 4)
            storeInt(p, value.value)
        }
        is IgniteDbValue.DbLong -> {
            storeByte(base, 5)
            storeLong(p, value.value)
        }
        is IgniteDbValue.DbFloat -> {
            storeByte(base, 6)
            storeFloat(p, value.value)
        }
        is IgniteDbValue.DbDouble -> {
            storeByte(base, 7)
            storeDouble(p, value.value)
        }
        is IgniteDbValue.DbChar -> {
            storeByte(base, 8)
            storeShort(p, value.value.code.toShort())
        }
        is IgniteDbValue.DbString -> {
            storeByte(base, 9)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is IgniteDbValue.DbUuid -> {
            storeByte(base, 10)
            storeLong(p, value.highBits)
            storeLong(p + 8, value.lowBits)
        }
        is IgniteDbValue.DbDate -> {
            storeByte(base, 11)
            storeLong(p, value.millis)
        }
        is IgniteDbValue.DbTimestamp -> {
            storeByte(base, 12)
            storeLong(p, value.millis)
            storeInt(p + 8, value.nanos)
        }
        is IgniteDbValue.DbTime -> {
            storeByte(base, 13)
            storeLong(p, value.nanos)
        }
        is IgniteDbValue.DbDecimal -> {
            storeByte(base, 14)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is IgniteDbValue.DbByteArray -> {
            storeByte(base, 15)
            lowerListOfUByte(p, value.value)
        }
    }
}

private fun lowerIgniteDbValueList(values: List<IgniteDbValue>): Pair<Int, Int> {
    val arr = alloc(values.size * IGNITE_DBV_SIZE, IGNITE_DBV_ALIGN)
    values.forEachIndexed { i, v -> lowerIgniteDbValue(arr + i * IGNITE_DBV_SIZE, v) }
    return arr to values.size
}

/** Matches `golem:rdbms/ignite2@1.5.0`'s `db-column` record -- no type-name field at all (matching Scala's own `IgniteDbColumn`, which likewise omits it). */
data class IgniteDbColumn(val ordinal: Long, val name: String)

/** See [PostgresDbValue.asDisplayString]'s doc comment: same `v.toString`-bug fix applies here. */
private fun IgniteDbValue.asDisplayString(): String = when (this) {
    is IgniteDbValue.DbByte -> value.toString()
    is IgniteDbValue.DbShort -> value.toString()
    is IgniteDbValue.DbInt -> value.toString()
    is IgniteDbValue.DbLong -> value.toString()
    is IgniteDbValue.DbFloat -> value.toString()
    is IgniteDbValue.DbDouble -> value.toString()
    is IgniteDbValue.DbBoolean -> value.toString()
    is IgniteDbValue.DbString -> value
    is IgniteDbValue.DbDecimal -> value
    is IgniteDbValue.DbChar -> value.toString()
    else -> toString()
}

/** Mirrors Scala's `IgniteDbRow` in full (`getString`/`getInt`/`getLong`, same as `PostgresDbRow`). */
data class IgniteDbRow(val values: List<IgniteDbValue>) {
    fun getString(index: Int): String? = when (val v = values[index]) {
        IgniteDbValue.DbNull -> null
        else -> v.asDisplayString()
    }

    fun getInt(index: Int): Int? = when (val v = values[index]) {
        IgniteDbValue.DbNull -> null
        is IgniteDbValue.DbInt -> v.value
        is IgniteDbValue.DbByte -> v.value.toInt()
        is IgniteDbValue.DbShort -> v.value.toInt()
        else -> v.asDisplayString().toInt()
    }

    fun getLong(index: Int): Long? = when (val v = values[index]) {
        IgniteDbValue.DbNull -> null
        is IgniteDbValue.DbLong -> v.value
        is IgniteDbValue.DbInt -> v.value.toLong()
        else -> v.asDisplayString().toLong()
    }
}

data class IgniteDbResult(val columns: List<IgniteDbColumn>, val rows: List<IgniteDbRow>)

// ignite db-column: size=16 align=8 { ordinal@0(8,8), name@8(8,4) }.
private fun liftIgniteDbColumn(base: Int): IgniteDbColumn = IgniteDbColumn(loadLong(base), liftString(loadInt(base + 8), loadInt(base + 12)))

// ignite db-row: size=8 align=4 { values: list<db-value> @0 }.
private fun liftIgniteDbRow(base: Int): IgniteDbRow {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return IgniteDbRow((0 until len).map { i -> liftIgniteDbValue(ptr + i * IGNITE_DBV_SIZE) })
}

// ignite db-result: size=16 align=4 { columns: list<db-column> @0, rows: list<db-row> @8 }.
private fun liftIgniteDbResult(base: Int): IgniteDbResult {
    val colPtr = loadInt(base)
    val colLen = loadInt(base + 4)
    val rowPtr = loadInt(base + 8)
    val rowLen = loadInt(base + 12)
    return IgniteDbResult(
        columns = (0 until colLen).map { i -> liftIgniteDbColumn(colPtr + i * 16) },
        rows = (0 until rowLen).map { i -> liftIgniteDbRow(rowPtr + i * 8) },
    )
}

/** Matches `golem:rdbms/ignite2@1.5.0`'s `error` variant (6 cases, all string payloads). */
sealed class IgniteDbError {
    data class ConnectionFailure(val message: String) : IgniteDbError()
    data class QueryParameterFailure(val message: String) : IgniteDbError()
    data class QueryExecutionFailure(val message: String) : IgniteDbError()
    data class QueryResponseFailure(val message: String) : IgniteDbError()
    data class Other(val message: String) : IgniteDbError()

    /** The resident transaction or result stream cannot be resumed after a restart; recorded results stay valid. Added in Golem 1.6.0. */
    data class ResourceLost(val message: String) : IgniteDbError()
}

private fun liftIgniteDbError(base: Int): IgniteDbError {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + 4
    val msg = liftString(loadInt(p), loadInt(p + 4))
    return when (tag) {
        0 -> IgniteDbError.ConnectionFailure(msg)
        1 -> IgniteDbError.QueryParameterFailure(msg)
        2 -> IgniteDbError.QueryExecutionFailure(msg)
        3 -> IgniteDbError.QueryResponseFailure(msg)
        4 -> IgniteDbError.Other(msg)
        5 -> IgniteDbError.ResourceLost(msg)
        else -> error("unknown golem:rdbms ignite error tag $tag")
    }
}

/** A live connection to an Apache Ignite 2.x node. MUST be [close]d when done. */
class IgniteConnection internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, IgniteDbResult> {
        check(!closed) { "IgniteConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerIgniteDbValueList(params)
        val retPtr = alloc(20, 4) // result<db-result, error>: tag@0(1,1), payload@4(max(16,12)=16)
        hostIgniteConnectionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftIgniteDbResult(retPtr + 4)) else Either.Left(liftIgniteDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, Long> {
        check(!closed) { "IgniteConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerIgniteDbValueList(params)
        val retPtr = alloc(24, 8) // result<s64, error>: tag@0(1,1), payload@8 (rounded to align8)
        hostIgniteConnectionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftIgniteDbError(retPtr + 8))
    }

    fun beginTransaction(): Either<IgniteDbError, IgniteTransaction> {
        check(!closed) { "IgniteConnection already closed" }
        val retPtr = alloc(16, 4) // result<db-transaction, error>: tag@0(1,1), payload@4(max(4,12)=12)
        hostIgniteConnectionBeginTransaction(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(IgniteTransaction(loadInt(retPtr + 4))) else Either.Left(liftIgniteDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostIgniteConnectionDrop(handle)
            closed = true
        }
    }

    companion object {
        fun open(address: String): Either<IgniteDbError, IgniteConnection> {
            val (ptr, len) = lowerStringToPtrLen(address)
            val retPtr = alloc(16, 4)
            hostIgniteConnectionOpen(ptr, len, retPtr)
            return if (loadByte(retPtr).toInt() == 0) Either.Right(IgniteConnection(loadInt(retPtr + 4))) else Either.Left(liftIgniteDbError(retPtr + 4))
        }
    }
}

/** An open transaction on an [IgniteConnection]. MUST be [close]d when done, whether or not [commit]/[rollback] was called. */
class IgniteTransaction internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, IgniteDbResult> {
        check(!closed) { "IgniteTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerIgniteDbValueList(params)
        val retPtr = alloc(20, 4)
        hostIgniteTransactionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftIgniteDbResult(retPtr + 4)) else Either.Left(liftIgniteDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, Long> {
        check(!closed) { "IgniteTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerIgniteDbValueList(params)
        val retPtr = alloc(24, 8)
        hostIgniteTransactionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftIgniteDbError(retPtr + 8))
    }

    fun commit(): Either<IgniteDbError, Unit> {
        check(!closed) { "IgniteTransaction already closed" }
        val retPtr = alloc(16, 4)
        hostIgniteTransactionCommit(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftIgniteDbError(retPtr + 4))
    }

    fun rollback(): Either<IgniteDbError, Unit> {
        check(!closed) { "IgniteTransaction already closed" }
        val retPtr = alloc(16, 4)
        hostIgniteTransactionRollback(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftIgniteDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostIgniteTransactionDrop(handle)
            closed = true
        }
    }
}

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}
