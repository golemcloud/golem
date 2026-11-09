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
// MySQL (golem:rdbms/mysql@1.5.0)
// ===========================================================================
//
// Entirely flat -- unlike Postgres, MySQL's db-value has no lazy-db-value-style recursive
// cases at all (no composite/domain/array/range equivalents in this WIT interface), so this
// port is complete in one increment, mirroring Postgres's Connection/Transaction/query/
// execute/beginTransaction/commit/rollback shape exactly. Signatures verified via abi-dump.

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[static]db-connection.open")
private external fun hostMysqlConnectionOpen(addrPtr: Int, addrLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-connection.query")
private external fun hostMysqlConnectionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-connection.execute")
private external fun hostMysqlConnectionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-connection.begin-transaction")
private external fun hostMysqlConnectionBeginTransaction(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[resource-drop]db-connection")
private external fun hostMysqlConnectionDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-transaction.query")
private external fun hostMysqlTransactionQuery(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-transaction.execute")
private external fun hostMysqlTransactionExecute(handle: Int, stmtPtr: Int, stmtLen: Int, paramsPtr: Int, paramsLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-transaction.commit")
private external fun hostMysqlTransactionCommit(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[method]db-transaction.rollback")
private external fun hostMysqlTransactionRollback(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:rdbms/mysql@1.5.0", "[resource-drop]db-transaction")
private external fun hostMysqlTransactionDrop(handle: Int)

/** Matches `golem:rdbms/mysql@1.5.0`'s `db-value` variant in full (36 cases, tags 0-35). */
sealed class MysqlDbValue {
    data class BooleanVal(val value: Boolean) : MysqlDbValue()
    data class TinyInt(val value: Byte) : MysqlDbValue()
    data class SmallInt(val value: Short) : MysqlDbValue()
    data class MediumInt(val value: Int) : MysqlDbValue()
    data class IntVal(val value: Int) : MysqlDbValue()
    data class BigInt(val value: Long) : MysqlDbValue()
    data class TinyIntUnsigned(val value: UByte) : MysqlDbValue()
    data class SmallIntUnsigned(val value: UShort) : MysqlDbValue()
    data class MediumIntUnsigned(val value: UInt) : MysqlDbValue()
    data class IntUnsigned(val value: UInt) : MysqlDbValue()
    data class BigIntUnsigned(val value: ULong) : MysqlDbValue()
    data class FloatVal(val value: Float) : MysqlDbValue()
    data class DoubleVal(val value: Double) : MysqlDbValue()

    /** WIT represents `decimal` as an arbitrary-precision decimal encoded as a string. */
    data class Decimal(val value: String) : MysqlDbValue()
    data class DateVal(val value: DbDate) : MysqlDbValue()
    data class DateTimeVal(val value: DbTimestamp) : MysqlDbValue()
    data class TimestampVal(val value: DbTimestamp) : MysqlDbValue()
    data class TimeVal(val value: DbTime) : MysqlDbValue()
    data class Year(val value: UShort) : MysqlDbValue()
    data class FixChar(val value: String) : MysqlDbValue()
    data class VarChar(val value: String) : MysqlDbValue()
    data class TinyText(val value: String) : MysqlDbValue()
    data class Text(val value: String) : MysqlDbValue()
    data class MediumText(val value: String) : MysqlDbValue()
    data class LongText(val value: String) : MysqlDbValue()
    data class Binary(val value: List<UByte>) : MysqlDbValue()
    data class VarBinary(val value: List<UByte>) : MysqlDbValue()
    data class TinyBlob(val value: List<UByte>) : MysqlDbValue()
    data class Blob(val value: List<UByte>) : MysqlDbValue()
    data class MediumBlob(val value: List<UByte>) : MysqlDbValue()
    data class LongBlob(val value: List<UByte>) : MysqlDbValue()
    data class Enumeration(val value: String) : MysqlDbValue()
    data class SetVal(val value: String) : MysqlDbValue()
    data class Bit(val value: List<Boolean>) : MysqlDbValue()
    data class Json(val value: String) : MysqlDbValue()
    object Null : MysqlDbValue()
}

// db-value: size=24 align=8, tag_size=1, payload_offset=8.
private const val MYSQL_DBV_SIZE = 24
private const val MYSQL_DBV_ALIGN = 8
private const val MYSQL_DBV_PAYLOAD_OFFSET = 8

internal fun liftListOfUByte(base: Int): List<UByte> {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> loadByte(ptr + i).toUByte() }
}
internal fun lowerListOfUByte(base: Int, bytes: List<UByte>) {
    val arr = alloc(bytes.size, 1)
    bytes.forEachIndexed { i, b -> storeByte(arr + i, b.toByte()) }
    storeInt(base, arr)
    storeInt(base + 4, bytes.size)
}

private fun liftMysqlDbValue(base: Int): MysqlDbValue {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + MYSQL_DBV_PAYLOAD_OFFSET
    return when (tag) {
        0 -> MysqlDbValue.BooleanVal(loadByte(p).toInt() != 0)
        1 -> MysqlDbValue.TinyInt(loadByte(p))
        2 -> MysqlDbValue.SmallInt(loadShort(p))
        3 -> MysqlDbValue.MediumInt(loadInt(p))
        4 -> MysqlDbValue.IntVal(loadInt(p))
        5 -> MysqlDbValue.BigInt(loadLong(p))
        6 -> MysqlDbValue.TinyIntUnsigned(loadByte(p).toUByte())
        7 -> MysqlDbValue.SmallIntUnsigned(loadShort(p).toUShort())
        8 -> MysqlDbValue.MediumIntUnsigned(loadInt(p).toUInt())
        9 -> MysqlDbValue.IntUnsigned(loadInt(p).toUInt())
        10 -> MysqlDbValue.BigIntUnsigned(loadLong(p).toULong())
        11 -> MysqlDbValue.FloatVal(loadFloat(p))
        12 -> MysqlDbValue.DoubleVal(loadDouble(p))
        13 -> MysqlDbValue.Decimal(liftString(loadInt(p), loadInt(p + 4)))
        14 -> MysqlDbValue.DateVal(liftDbDate(p))
        15 -> MysqlDbValue.DateTimeVal(liftDbTimestamp(p))
        16 -> MysqlDbValue.TimestampVal(liftDbTimestamp(p))
        17 -> MysqlDbValue.TimeVal(liftDbTime(p))
        18 -> MysqlDbValue.Year(loadShort(p).toUShort())
        19 -> MysqlDbValue.FixChar(liftString(loadInt(p), loadInt(p + 4)))
        20 -> MysqlDbValue.VarChar(liftString(loadInt(p), loadInt(p + 4)))
        21 -> MysqlDbValue.TinyText(liftString(loadInt(p), loadInt(p + 4)))
        22 -> MysqlDbValue.Text(liftString(loadInt(p), loadInt(p + 4)))
        23 -> MysqlDbValue.MediumText(liftString(loadInt(p), loadInt(p + 4)))
        24 -> MysqlDbValue.LongText(liftString(loadInt(p), loadInt(p + 4)))
        25 -> MysqlDbValue.Binary(liftListOfUByte(p))
        26 -> MysqlDbValue.VarBinary(liftListOfUByte(p))
        27 -> MysqlDbValue.TinyBlob(liftListOfUByte(p))
        28 -> MysqlDbValue.Blob(liftListOfUByte(p))
        29 -> MysqlDbValue.MediumBlob(liftListOfUByte(p))
        30 -> MysqlDbValue.LongBlob(liftListOfUByte(p))
        31 -> MysqlDbValue.Enumeration(liftString(loadInt(p), loadInt(p + 4)))
        32 -> MysqlDbValue.SetVal(liftString(loadInt(p), loadInt(p + 4)))
        33 -> MysqlDbValue.Bit(liftListOfBool(p))
        34 -> MysqlDbValue.Json(liftString(loadInt(p), loadInt(p + 4)))
        35 -> MysqlDbValue.Null
        else -> error("native Rdbms: unknown mysql db-value tag $tag")
    }
}

private fun lowerMysqlDbValue(base: Int, value: MysqlDbValue) {
    val p = base + MYSQL_DBV_PAYLOAD_OFFSET
    when (value) {
        is MysqlDbValue.BooleanVal -> {
            storeByte(base, 0)
            storeByte(p, if (value.value) 1 else 0)
        }
        is MysqlDbValue.TinyInt -> {
            storeByte(base, 1)
            storeByte(p, value.value)
        }
        is MysqlDbValue.SmallInt -> {
            storeByte(base, 2)
            storeShort(p, value.value)
        }
        is MysqlDbValue.MediumInt -> {
            storeByte(base, 3)
            storeInt(p, value.value)
        }
        is MysqlDbValue.IntVal -> {
            storeByte(base, 4)
            storeInt(p, value.value)
        }
        is MysqlDbValue.BigInt -> {
            storeByte(base, 5)
            storeLong(p, value.value)
        }
        is MysqlDbValue.TinyIntUnsigned -> {
            storeByte(base, 6)
            storeByte(p, value.value.toByte())
        }
        is MysqlDbValue.SmallIntUnsigned -> {
            storeByte(base, 7)
            storeShort(p, value.value.toShort())
        }
        is MysqlDbValue.MediumIntUnsigned -> {
            storeByte(base, 8)
            storeInt(p, value.value.toInt())
        }
        is MysqlDbValue.IntUnsigned -> {
            storeByte(base, 9)
            storeInt(p, value.value.toInt())
        }
        is MysqlDbValue.BigIntUnsigned -> {
            storeByte(base, 10)
            storeLong(p, value.value.toLong())
        }
        is MysqlDbValue.FloatVal -> {
            storeByte(base, 11)
            storeFloat(p, value.value)
        }
        is MysqlDbValue.DoubleVal -> {
            storeByte(base, 12)
            storeDouble(p, value.value)
        }
        is MysqlDbValue.Decimal -> {
            storeByte(base, 13)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.DateVal -> {
            storeByte(base, 14)
            storeDbDate(p, value.value)
        }
        is MysqlDbValue.DateTimeVal -> {
            storeByte(base, 15)
            storeDbTimestamp(p, value.value)
        }
        is MysqlDbValue.TimestampVal -> {
            storeByte(base, 16)
            storeDbTimestamp(p, value.value)
        }
        is MysqlDbValue.TimeVal -> {
            storeByte(base, 17)
            storeDbTime(p, value.value)
        }
        is MysqlDbValue.Year -> {
            storeByte(base, 18)
            storeShort(p, value.value.toShort())
        }
        is MysqlDbValue.FixChar -> {
            storeByte(base, 19)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.VarChar -> {
            storeByte(base, 20)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.TinyText -> {
            storeByte(base, 21)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.Text -> {
            storeByte(base, 22)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.MediumText -> {
            storeByte(base, 23)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.LongText -> {
            storeByte(base, 24)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.Binary -> {
            storeByte(base, 25)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.VarBinary -> {
            storeByte(base, 26)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.TinyBlob -> {
            storeByte(base, 27)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.Blob -> {
            storeByte(base, 28)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.MediumBlob -> {
            storeByte(base, 29)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.LongBlob -> {
            storeByte(base, 30)
            lowerListOfUByte(p, value.value)
        }
        is MysqlDbValue.Enumeration -> {
            storeByte(base, 31)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.SetVal -> {
            storeByte(base, 32)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        is MysqlDbValue.Bit -> {
            storeByte(base, 33)
            lowerListOfBool(p, value.value)
        }
        is MysqlDbValue.Json -> {
            storeByte(base, 34)
            val (ptr, len) = lowerStringToPtrLen(value.value)
            storeInt(p, ptr)
            storeInt(p + 4, len)
        }
        MysqlDbValue.Null -> storeByte(base, 35)
    }
}

private fun lowerMysqlDbValueList(values: List<MysqlDbValue>): Pair<Int, Int> {
    val arr = alloc(values.size * MYSQL_DBV_SIZE, MYSQL_DBV_ALIGN)
    values.forEachIndexed { i, v -> lowerMysqlDbValue(arr + i * MYSQL_DBV_SIZE, v) }
    return arr to values.size
}

/** Mirrors Scala's own `DbColumn` for MySQL: only `dbTypeName`, no structural `db-type` (mysql's `db-column-type` is a plain 1-byte tag-only enum, cheaper to skip than to decode for no consumer). */
data class MysqlDbColumn(val ordinal: Long, val name: String, val dbTypeName: String)

/**
 * See [PostgresDbValue.asDisplayString]'s doc comment: same reasoning applies here -- Scala's
 * `MysqlDbRow.getString`/`getInt` fall back to `v.toString` with no `MysqlDbValue` override,
 * so this fixes the value extraction instead of reproducing that bug.
 */
private fun MysqlDbValue.asDisplayString(): String = when (this) {
    is MysqlDbValue.TinyInt -> value.toString()
    is MysqlDbValue.SmallInt -> value.toString()
    is MysqlDbValue.MediumInt -> value.toString()
    is MysqlDbValue.IntVal -> value.toString()
    is MysqlDbValue.BigInt -> value.toString()
    is MysqlDbValue.TinyIntUnsigned -> value.toString()
    is MysqlDbValue.SmallIntUnsigned -> value.toString()
    is MysqlDbValue.MediumIntUnsigned -> value.toString()
    is MysqlDbValue.IntUnsigned -> value.toString()
    is MysqlDbValue.BigIntUnsigned -> value.toString()
    is MysqlDbValue.FloatVal -> value.toString()
    is MysqlDbValue.DoubleVal -> value.toString()
    is MysqlDbValue.Decimal -> value
    is MysqlDbValue.BooleanVal -> value.toString()
    is MysqlDbValue.FixChar -> value
    is MysqlDbValue.VarChar -> value
    is MysqlDbValue.TinyText -> value
    is MysqlDbValue.Text -> value
    is MysqlDbValue.MediumText -> value
    is MysqlDbValue.LongText -> value
    is MysqlDbValue.Enumeration -> value
    is MysqlDbValue.SetVal -> value
    is MysqlDbValue.Json -> value
    is MysqlDbValue.Year -> value.toString()
    else -> toString()
}

/** Mirrors Scala's `MysqlDbRow` -- only `getString`/`getInt` (Scala's own doesn't expose `getLong`). */
data class MysqlDbRow(val values: List<MysqlDbValue>) {
    fun getString(index: Int): String? = when (val v = values[index]) {
        MysqlDbValue.Null -> null
        else -> v.asDisplayString()
    }

    fun getInt(index: Int): Int? = when (val v = values[index]) {
        MysqlDbValue.Null -> null
        is MysqlDbValue.IntVal -> v.value
        is MysqlDbValue.TinyInt -> v.value.toInt()
        is MysqlDbValue.SmallInt -> v.value.toInt()
        is MysqlDbValue.MediumInt -> v.value
        else -> v.asDisplayString().toInt()
    }
}

data class MysqlDbResult(val columns: List<MysqlDbColumn>, val rows: List<MysqlDbRow>)

// mysql db-column: size=32 align=8 { ordinal@0(8,8), name@8(8,4), db-type@16(1,1, skipped), db-type-name@20(8,4) }.
private fun liftMysqlDbColumn(base: Int): MysqlDbColumn = MysqlDbColumn(
    ordinal = loadLong(base),
    name = liftString(loadInt(base + 8), loadInt(base + 12)),
    dbTypeName = liftString(loadInt(base + 20), loadInt(base + 24)),
)

// mysql db-row: size=8 align=4 { values: list<db-value> @0 }.
private fun liftMysqlDbRow(base: Int): MysqlDbRow {
    val ptr = loadInt(base)
    val len = loadInt(base + 4)
    return MysqlDbRow((0 until len).map { i -> liftMysqlDbValue(ptr + i * MYSQL_DBV_SIZE) })
}

// mysql db-result: size=16 align=4 { columns: list<db-column> @0, rows: list<db-row> @8 }.
private fun liftMysqlDbResult(base: Int): MysqlDbResult {
    val colPtr = loadInt(base)
    val colLen = loadInt(base + 4)
    val rowPtr = loadInt(base + 8)
    val rowLen = loadInt(base + 12)
    return MysqlDbResult(
        columns = (0 until colLen).map { i -> liftMysqlDbColumn(colPtr + i * 32) },
        rows = (0 until rowLen).map { i -> liftMysqlDbRow(rowPtr + i * 8) },
    )
}

/** Matches `golem:rdbms/mysql@1.5.0`'s `error` variant (6 cases, all string payloads). */
sealed class MysqlDbError {
    data class ConnectionFailure(val message: String) : MysqlDbError()
    data class QueryParameterFailure(val message: String) : MysqlDbError()
    data class QueryExecutionFailure(val message: String) : MysqlDbError()
    data class QueryResponseFailure(val message: String) : MysqlDbError()
    data class Other(val message: String) : MysqlDbError()

    /** The resident transaction or result stream cannot be resumed after a restart; recorded results stay valid. Added in Golem 1.6.0. */
    data class ResourceLost(val message: String) : MysqlDbError()
}

private fun liftMysqlDbError(base: Int): MysqlDbError {
    val tag = loadByte(base).toInt() and 0xFF
    val p = base + 4
    val msg = liftString(loadInt(p), loadInt(p + 4))
    return when (tag) {
        0 -> MysqlDbError.ConnectionFailure(msg)
        1 -> MysqlDbError.QueryParameterFailure(msg)
        2 -> MysqlDbError.QueryExecutionFailure(msg)
        3 -> MysqlDbError.QueryResponseFailure(msg)
        4 -> MysqlDbError.Other(msg)
        5 -> MysqlDbError.ResourceLost(msg)
        else -> error("unknown golem:rdbms mysql error tag $tag")
    }
}

/** A live connection to a MySQL database. MUST be [close]d when done. */
class MysqlConnection internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, MysqlDbResult> {
        check(!closed) { "MysqlConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerMysqlDbValueList(params)
        val retPtr = alloc(20, 4) // result<db-result, error>: tag@0(1,1), payload@4(max(16,12)=16)
        hostMysqlConnectionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftMysqlDbResult(retPtr + 4)) else Either.Left(liftMysqlDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, Long> {
        check(!closed) { "MysqlConnection already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerMysqlDbValueList(params)
        val retPtr = alloc(24, 8) // result<u64, error>: tag@0(1,1), payload@8 (rounded to align8)
        hostMysqlConnectionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftMysqlDbError(retPtr + 8))
    }

    fun beginTransaction(): Either<MysqlDbError, MysqlTransaction> {
        check(!closed) { "MysqlConnection already closed" }
        val retPtr = alloc(16, 4) // result<db-transaction, error>: tag@0(1,1), payload@4(max(4,12)=12)
        hostMysqlConnectionBeginTransaction(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(MysqlTransaction(loadInt(retPtr + 4))) else Either.Left(liftMysqlDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostMysqlConnectionDrop(handle)
            closed = true
        }
    }

    companion object {
        fun open(address: String): Either<MysqlDbError, MysqlConnection> {
            val (ptr, len) = lowerStringToPtrLen(address)
            val retPtr = alloc(16, 4)
            hostMysqlConnectionOpen(ptr, len, retPtr)
            return if (loadByte(retPtr).toInt() == 0) Either.Right(MysqlConnection(loadInt(retPtr + 4))) else Either.Left(liftMysqlDbError(retPtr + 4))
        }
    }
}

/** An open transaction on a [MysqlConnection]. MUST be [close]d when done, whether or not [commit]/[rollback] was called. */
class MysqlTransaction internal constructor(private val handle: Int) {
    private var closed = false

    fun query(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, MysqlDbResult> {
        check(!closed) { "MysqlTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerMysqlDbValueList(params)
        val retPtr = alloc(20, 4)
        hostMysqlTransactionQuery(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(liftMysqlDbResult(retPtr + 4)) else Either.Left(liftMysqlDbError(retPtr + 4))
    }

    fun execute(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, Long> {
        check(!closed) { "MysqlTransaction already closed" }
        val (stmtPtr, stmtLen) = lowerStringToPtrLen(statement)
        val (paramsPtr, paramsLen) = lowerMysqlDbValueList(params)
        val retPtr = alloc(24, 8)
        hostMysqlTransactionExecute(handle, stmtPtr, stmtLen, paramsPtr, paramsLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(loadLong(retPtr + 8)) else Either.Left(liftMysqlDbError(retPtr + 8))
    }

    fun commit(): Either<MysqlDbError, Unit> {
        check(!closed) { "MysqlTransaction already closed" }
        val retPtr = alloc(16, 4)
        hostMysqlTransactionCommit(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftMysqlDbError(retPtr + 4))
    }

    fun rollback(): Either<MysqlDbError, Unit> {
        check(!closed) { "MysqlTransaction already closed" }
        val retPtr = alloc(16, 4)
        hostMysqlTransactionRollback(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) Either.Right(Unit) else Either.Left(liftMysqlDbError(retPtr + 4))
    }

    fun close() {
        if (!closed) {
            hostMysqlTransactionDrop(handle)
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
