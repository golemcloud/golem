# RDBMS

> Relational-database bindings for **Postgres**, **MySQL**, and **Apache Ignite 2.x** over `golem:rdbms/postgres@1.5.0`, `golem:rdbms/mysql@1.5.0`, and `golem:rdbms/ignite2@1.5.0`. **Status:** Complete.

## Overview

The RDBMS bindings let a Golem agent open a durable connection to a relational database,
run parameterised statements, iterate typed rows, and drive transactions. Three backends are
supported, each behind its own set of Kotlin types:

| Backend | WIT package | Connection | Transaction | Value type | Error type | Result / Row / Column |
| --- | --- | --- | --- | --- | --- | --- |
| Postgres | `golem:rdbms/postgres@1.5.0` | `PostgresConnection` | `PostgresTransaction` | `PostgresDbValue` (45 cases) | `DbError` | `PostgresDbResult` / `PostgresDbRow` / `DbColumn` |
| MySQL | `golem:rdbms/mysql@1.5.0` | `MysqlConnection` | `MysqlTransaction` | `MysqlDbValue` (36 cases) | `MysqlDbError` | `MysqlDbResult` / `MysqlDbRow` / `MysqlDbColumn` |
| Ignite | `golem:rdbms/ignite2@1.5.0` | `IgniteConnection` | `IgniteTransaction` | `IgniteDbValue` (16 cases) | `IgniteDbError` | `IgniteDbResult` / `IgniteDbRow` / `IgniteDbColumn` |

All three backends share the **same shape**:

- `<Backend>Connection.open(address)` opens a connection; `query`, `execute`, and
  `beginTransaction` operate on it; `close()` releases it.
- `<Backend>Transaction` has `query`, `execute`, `commit`, `rollback`, and `close`.
- Every fallible call returns [`Either<Error, T>`](transactions.md#either) — `Either.Right`
  on success, `Either.Left(error)` on failure. No exceptions are thrown for database errors.
- `query` returns a fully-materialised result (columns + rows); `execute` returns the affected
  row count (`Long`).
- Parameters are passed as `List<<Backend>DbValue>` and default to `emptyList()`.

Connections and transactions are backed by host resource handles and **must be `close()`d**
when done (both classes guard against use-after-close with a `check(!closed)`).

> **Query streaming is out of scope.** The WIT interfaces expose a `query-stream` /
> `db-result-stream` resource for lazy, paginated results, but — matching the Scala reference
> SDK, which likewise does not implement it — the Kotlin SDK only exposes the eager `query`
> that materialises the whole `db-result`. Streaming a result incrementally is a separate,
> deliberately deferred capability.

For the SDK overview see [`../../README.md`](../../README.md). Related host APIs:
[Transactions](transactions.md) (the `Either` type and higher-level transaction combinators)
and [Quota](quota.md).

## Shared value model

Several column value cases are shared verbatim across backends (they live in
`golem:rdbms/types@1.5.0` and are reused by the Postgres/MySQL value variants):

```kotlin
/** golem:rdbms/types date record. */
data class DbDate(val year: Int, val month: UByte, val day: UByte)

/** golem:rdbms/types time record. */
data class DbTime(val hour: UByte, val minute: UByte, val second: UByte, val nanosecond: UInt)

/** golem:rdbms/types timestamp record. */
data class DbTimestamp(val date: DbDate, val time: DbTime)

/** golem:rdbms/types timestamptz record. */
data class DbTimestampTz(val timestamp: DbTimestamp, val offset: Int)

/** golem:rdbms/types timetz record. */
data class DbTimeTz(val time: DbTime, val offset: Int)
```

> Note: Ignite does **not** use these records — it encodes temporal values as raw
> milliseconds/nanoseconds (see [Ignite](#ignite)).

---

## Postgres

`golem:rdbms/postgres@1.5.0`. The richest of the three backends: 45 `db-value` cases including
temporal, network, bit-string, range, and pgvector types, plus the recursive composite / domain
/ array / range cases.

### `PostgresConnection`

```kotlin
class PostgresConnection {
    fun query(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, PostgresDbResult>
    fun execute(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, Long>
    fun beginTransaction(): Either<DbError, PostgresTransaction>
    fun close()

    companion object {
        fun open(address: String): Either<DbError, PostgresConnection>
    }
}
```

`execute` returns the number of affected rows (`u64`). The connection must be `close()`d.

### `PostgresTransaction`

```kotlin
class PostgresTransaction {
    fun query(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, PostgresDbResult>
    fun execute(statement: String, params: List<PostgresDbValue> = emptyList()): Either<DbError, Long>
    fun commit(): Either<DbError, Unit>
    fun rollback(): Either<DbError, Unit>
    fun close()
}
```

Must be `close()`d whether or not `commit`/`rollback` was called.

### Result, row, and column types

```kotlin
data class PostgresDbResult(val columns: List<DbColumn>, val rows: List<PostgresDbRow>)

/** Mirrors Scala's DbColumn: carries only dbTypeName, not the structural column type. */
data class DbColumn(val ordinal: Long, val name: String, val dbTypeName: String)

data class PostgresDbRow(val values: List<PostgresDbValue>) {
    fun getString(index: Int): String?   // Null -> null; text/numeric cases return raw content
    fun getInt(index: Int): Int?         // Null -> null; Int4/Int2 fast path, else parses display string
    fun getLong(index: Int): Long?       // Null -> null; Int8/Int4 fast path, else parses display string
}
```

The row accessors return `null` for SQL `NULL`. `getString` returns the raw string content for
textual and numeric-string cases (`Text`/`Varchar`/`Bpchar`/`Numeric`/`Json`/`Jsonb`/`JsonPath`/`Xml`);
other cases fall back to a structural `toString()` dump.

### Error type

```kotlin
sealed class DbError {
    data class ConnectionFailure(val message: String) : DbError()
    data class QueryParameterFailure(val message: String) : DbError()
    data class QueryExecutionFailure(val message: String) : DbError()
    data class QueryResponseFailure(val message: String) : DbError()
    data class Other(val message: String) : DbError()
}
```

### `PostgresDbValue` — column value coverage

All 45 cases (used both as query parameters and as row values):

```kotlin
sealed class PostgresDbValue {
    // Numeric / boolean
    data class Character(val value: Byte) : PostgresDbValue()   // "char"
    data class Int2(val value: Short) : PostgresDbValue()
    data class Int4(val value: Int) : PostgresDbValue()
    data class Int8(val value: Long) : PostgresDbValue()
    data class Float4(val value: Float) : PostgresDbValue()
    data class Float8(val value: Double) : PostgresDbValue()
    data class Numeric(val value: String) : PostgresDbValue()   // arbitrary-precision decimal as string
    data class BooleanVal(val value: Boolean) : PostgresDbValue()
    data class Money(val value: Long) : PostgresDbValue()
    data class Oid(val value: UInt) : PostgresDbValue()

    // Text
    data class Text(val value: String) : PostgresDbValue()
    data class Varchar(val value: String) : PostgresDbValue()
    data class Bpchar(val value: String) : PostgresDbValue()
    data class Json(val value: String) : PostgresDbValue()
    data class Jsonb(val value: String) : PostgresDbValue()
    data class JsonPath(val value: String) : PostgresDbValue()
    data class Xml(val value: String) : PostgresDbValue()

    // Temporal (shared records)
    data class TimestampVal(val value: DbTimestamp) : PostgresDbValue()
    data class TimestampTzVal(val value: DbTimestampTz) : PostgresDbValue()
    data class DateVal(val value: DbDate) : PostgresDbValue()
    data class TimeVal(val value: DbTime) : PostgresDbValue()
    data class TimeTzVal(val value: DbTimeTz) : PostgresDbValue()
    data class IntervalVal(val value: DbInterval) : PostgresDbValue()

    // Binary / identity
    data class Bytea(val value: List<UByte>) : PostgresDbValue()
    data class Uuid(val highBits: Long, val lowBits: Long) : PostgresDbValue()

    // Network
    data class InetVal(val value: IpAddress) : PostgresDbValue()
    data class CidrVal(val value: IpAddress) : PostgresDbValue()
    data class MacaddrVal(val value: MacAddress) : PostgresDbValue()

    // Bit strings
    data class Bit(val value: List<Boolean>) : PostgresDbValue()
    data class Varbit(val value: List<Boolean>) : PostgresDbValue()

    // Ranges
    data class Int4RangeVal(val value: Int4Range) : PostgresDbValue()
    data class Int8RangeVal(val value: Int8Range) : PostgresDbValue()
    data class NumRangeVal(val value: NumRange) : PostgresDbValue()
    data class TsRangeVal(val value: TsRange) : PostgresDbValue()
    data class TsTzRangeVal(val value: TsTzRange) : PostgresDbValue()
    data class DateRangeVal(val value: DateRange) : PostgresDbValue()

    // Composite / user-defined (recursive — see below)
    data class EnumerationVal(val value: PostgresEnumeration) : PostgresDbValue()
    data class CompositeVal(val value: PostgresComposite) : PostgresDbValue()
    data class DomainVal(val value: PostgresDomain) : PostgresDbValue()
    data class ArrayVal(val value: List<PostgresDbValue>) : PostgresDbValue()
    data class RangeVal(val value: PostgresRange) : PostgresDbValue()

    // pgvector
    data class VectorVal(val value: List<Float>) : PostgresDbValue()
    data class HalfvecVal(val value: List<Float>) : PostgresDbValue()
    data class SparsevecVal(val value: SparseVec) : PostgresDbValue()

    object Null : PostgresDbValue()
}
```

Supporting types for the composite / temporal / network / range cases:

```kotlin
data class DbInterval(val months: Int, val days: Int, val microseconds: Long)

sealed class IpAddress {
    data class Ipv4(val a: UByte, val b: UByte, val c: UByte, val d: UByte) : IpAddress()
    data class Ipv6(
        val a: UShort, val b: UShort, val c: UShort, val d: UShort,
        val e: UShort, val f: UShort, val g: UShort, val h: UShort,
    ) : IpAddress()
}
data class MacAddress(val a: UByte, val b: UByte, val c: UByte, val d: UByte, val e: UByte, val f: UByte)

data class SparseVec(val dim: Int, val indices: List<Int>, val values: List<Float>)

data class PostgresEnumeration(val name: String, val value: String)
data class PostgresComposite(val name: String, val values: List<PostgresDbValue>)
data class PostgresDomain(val name: String, val value: PostgresDbValue)

data class PostgresRange(val name: String, val value: ValuesRange)
data class ValuesRange(val start: ValueBound, val end: ValueBound)
sealed class ValueBound {
    data class Included(val value: PostgresDbValue) : ValueBound()
    data class Excluded(val value: PostgresDbValue) : ValueBound()
    object Unbounded : ValueBound()
}

// Typed range bounds (Int4Bound/Int8Bound/NumBound/TsBound/TsTzBound/DateBound each have
// Included(value) / Excluded(value) / Unbounded), plus the range pairs:
data class Int4Range(val start: Int4Bound, val end: Int4Bound)
data class Int8Range(val start: Int8Bound, val end: Int8Bound)
data class NumRange(val start: NumBound, val end: NumBound)
data class TsRange(val start: TsBound, val end: TsBound)
data class TsTzRange(val start: TsTzBound, val end: TsTzBound)
data class DateRange(val start: DateBound, val end: DateBound)
```

#### The `lazy-db-value` resource (recursive cases)

Postgres's `enumeration` / `composite` / `domain` / `array` / `range` cases nest other
`db-value`s. At the WIT level these nested values go through a **`lazy-db-value` resource**
(`constructor(value: db-value); get() -> db-value`) — the recursion is indirect, through
resource handles in the host's table, which is why a single `db-value` has a bounded size on
the wire despite being conceptually recursive.

The SDK hides this entirely: nested values are **fully materialised** into plain Kotlin data
(`PostgresComposite.values` is a `List<PostgresDbValue>`, `PostgresDomain.value` is a
`PostgresDbValue`, `ArrayVal` is a `List<PostgresDbValue>`, and so on). Callers never see a
handle — real composite/array/domain/range nesting is finite, so eager recursive resolution is
the right default.

---

## MySQL

`golem:rdbms/mysql@1.5.0`. Entirely **flat** — MySQL's `db-value` has no recursive cases, so
there is no `lazy-db-value` involvement. The Connection/Transaction shape mirrors Postgres
exactly.

### Connection and transaction

```kotlin
class MysqlConnection {
    fun query(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, MysqlDbResult>
    fun execute(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, Long>
    fun beginTransaction(): Either<MysqlDbError, MysqlTransaction>
    fun close()
    companion object { fun open(address: String): Either<MysqlDbError, MysqlConnection> }
}

class MysqlTransaction {
    fun query(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, MysqlDbResult>
    fun execute(statement: String, params: List<MysqlDbValue> = emptyList()): Either<MysqlDbError, Long>
    fun commit(): Either<MysqlDbError, Unit>
    fun rollback(): Either<MysqlDbError, Unit>
    fun close()
}
```

### Result, row, and column types

```kotlin
data class MysqlDbResult(val columns: List<MysqlDbColumn>, val rows: List<MysqlDbRow>)
data class MysqlDbColumn(val ordinal: Long, val name: String, val dbTypeName: String)

data class MysqlDbRow(val values: List<MysqlDbValue>) {
    fun getString(index: Int): String?   // Null -> null
    fun getInt(index: Int): Int?         // Null -> null; IntVal/TinyInt/SmallInt/MediumInt fast path
}
```

> Unlike Postgres and Ignite, `MysqlDbRow` exposes only `getString` / `getInt` (no `getLong`),
> matching the Scala reference.

### Error type

```kotlin
sealed class MysqlDbError {
    data class ConnectionFailure(val message: String) : MysqlDbError()
    data class QueryParameterFailure(val message: String) : MysqlDbError()
    data class QueryExecutionFailure(val message: String) : MysqlDbError()
    data class QueryResponseFailure(val message: String) : MysqlDbError()
    data class Other(val message: String) : MysqlDbError()
}
```

### `MysqlDbValue` — column value coverage

All 36 cases:

```kotlin
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
    data class Decimal(val value: String) : MysqlDbValue()      // arbitrary-precision as string
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
```

---

## Ignite

`golem:rdbms/ignite2@1.5.0`. Also flat, and the smallest backend (16 `db-value` cases). Two
differences worth noting: `IgniteDbColumn` has **no `dbTypeName` field** (matching Scala's
`IgniteDbColumn`), and temporal values are encoded as raw millisecond/nanosecond longs rather
than the shared `DbDate`/`DbTime`/`DbTimestamp` records. `execute` returns `s64`.

### Connection and transaction

```kotlin
class IgniteConnection {
    fun query(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, IgniteDbResult>
    fun execute(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, Long>
    fun beginTransaction(): Either<IgniteDbError, IgniteTransaction>
    fun close()
    companion object { fun open(address: String): Either<IgniteDbError, IgniteConnection> }
}

class IgniteTransaction {
    fun query(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, IgniteDbResult>
    fun execute(statement: String, params: List<IgniteDbValue> = emptyList()): Either<IgniteDbError, Long>
    fun commit(): Either<IgniteDbError, Unit>
    fun rollback(): Either<IgniteDbError, Unit>
    fun close()
}
```

### Result, row, and column types

```kotlin
data class IgniteDbResult(val columns: List<IgniteDbColumn>, val rows: List<IgniteDbRow>)
data class IgniteDbColumn(val ordinal: Long, val name: String)   // no type-name field

data class IgniteDbRow(val values: List<IgniteDbValue>) {
    fun getString(index: Int): String?   // DbNull -> null
    fun getInt(index: Int): Int?         // DbNull -> null; DbInt/DbByte/DbShort fast path
    fun getLong(index: Int): Long?       // DbNull -> null; DbLong/DbInt fast path
}
```

### Error type

```kotlin
sealed class IgniteDbError {
    data class ConnectionFailure(val message: String) : IgniteDbError()
    data class QueryParameterFailure(val message: String) : IgniteDbError()
    data class QueryExecutionFailure(val message: String) : IgniteDbError()
    data class QueryResponseFailure(val message: String) : IgniteDbError()
    data class Other(val message: String) : IgniteDbError()
}
```

### `IgniteDbValue` — column value coverage

All 16 cases:

```kotlin
sealed class IgniteDbValue {
    object DbNull : IgniteDbValue()
    data class DbBoolean(val value: Boolean) : IgniteDbValue()
    data class DbByte(val value: Byte) : IgniteDbValue()
    data class DbShort(val value: Short) : IgniteDbValue()
    data class DbInt(val value: Int) : IgniteDbValue()
    data class DbLong(val value: Long) : IgniteDbValue()
    data class DbFloat(val value: Float) : IgniteDbValue()
    data class DbDouble(val value: Double) : IgniteDbValue()
    data class DbChar(val value: Char) : IgniteDbValue()          // 16-bit Unicode code unit
    data class DbString(val value: String) : IgniteDbValue()
    data class DbUuid(val highBits: Long, val lowBits: Long) : IgniteDbValue()
    data class DbDate(val millis: Long) : IgniteDbValue()          // ms since Unix epoch (UTC)
    data class DbTimestamp(val millis: Long, val nanos: Int) : IgniteDbValue()  // ms + sub-ms nanos
    data class DbTime(val nanos: Long) : IgniteDbValue()           // nanos since midnight
    data class DbDecimal(val value: String) : IgniteDbValue()
    data class DbByteArray(val value: List<UByte>) : IgniteDbValue()
}
```

## Examples

All examples assume they run inside a Golem `@Agent` method. `Either.Left` carries the
backend's error type; `Either.Right` carries the success value.

### Open a connection and run a parameterised query (Postgres)

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.host.PostgresConnection
import cloud.golem.runtime.host.PostgresDbValue

fun lookupUserEmail(userId: Int): String? {
    val conn = when (val c = PostgresConnection.open("postgres://localhost/app")) {
        is Either.Left -> error("connect failed: ${c.value}")
        is Either.Right -> c.value
    }
    try {
        return when (val r = conn.query(
            "SELECT email FROM users WHERE id = $1",
            listOf(PostgresDbValue.Int4(userId)),
        )) {
            is Either.Left -> error("query failed: ${r.value}")
            is Either.Right -> r.value.rows.firstOrNull()?.getString(0)
        }
    } finally {
        conn.close()
    }
}
```

### Iterate rows and columns

```kotlin
fun listActiveUsers(conn: PostgresConnection): List<Pair<Long, String>> =
    when (val r = conn.query("SELECT id, name FROM users WHERE active = true")) {
        is Either.Left -> emptyList()
        is Either.Right -> r.value.rows.map { row ->
            (row.getLong(0) ?: 0L) to (row.getString(1) ?: "")
        }
    }
```

### An INSERT via `execute` (returns affected row count)

```kotlin
fun addUser(conn: PostgresConnection, name: String, email: String): Long =
    when (val r = conn.execute(
        "INSERT INTO users (name, email) VALUES ($1, $2)",
        listOf(PostgresDbValue.Text(name), PostgresDbValue.Text(email)),
    )) {
        is Either.Left -> 0L
        is Either.Right -> r.value  // number of rows affected
    }
```

### A transaction (Postgres)

```kotlin
fun transfer(conn: PostgresConnection, from: Int, to: Int, amount: Long): Either<DbError, Unit> {
    val tx = when (val t = conn.beginTransaction()) {
        is Either.Left -> return Either.Left(t.value)
        is Either.Right -> t.value
    }
    try {
        val debit = tx.execute(
            "UPDATE accounts SET balance = balance - $1 WHERE id = $2",
            listOf(PostgresDbValue.Int8(amount), PostgresDbValue.Int4(from)),
        )
        if (debit is Either.Left) { tx.rollback(); return Either.Left(debit.value) }

        val credit = tx.execute(
            "UPDATE accounts SET balance = balance + $1 WHERE id = $2",
            listOf(PostgresDbValue.Int8(amount), PostgresDbValue.Int4(to)),
        )
        if (credit is Either.Left) { tx.rollback(); return Either.Left(credit.value) }

        return tx.commit()
    } finally {
        tx.close()  // always release the handle, committed or not
    }
}
```

### MySQL and Ignite

The same patterns apply, swapping the backend types:

```kotlin
// MySQL — note '?' placeholders and MysqlDbValue
val conn = MysqlConnection.open("mysql://localhost/app")
// ... conn.query("SELECT name FROM users WHERE id = ?", listOf(MysqlDbValue.IntVal(42)))

// Ignite — IgniteDbValue, temporal values as raw longs
val ic = IgniteConnection.open("ignite://localhost:10800")
// ... ic.query("SELECT ts FROM events WHERE id = ?", listOf(IgniteDbValue.DbLong(1L)))
```

## Notes

- **Always `close()`** connections and transactions. Both guard against use-after-close and
  will throw `IllegalStateException` (via `check`) if reused after closing.
- **Errors are values, not exceptions.** Every fallible call returns `Either<Error, T>`; only
  programmer errors (use-after-close) throw.
- **`Null` is a value.** SQL `NULL` is represented by the backend's `Null` / `DbNull` case, and
  the row accessors (`getString`/`getInt`/`getLong`) map it to `null`.
- **`execute` return type:** affected row count as `Long` (`u64` for Postgres/MySQL, `s64` for
  Ignite).
- **`DbColumn` carries only `dbTypeName`** (Postgres/MySQL) or nothing but `ordinal`/`name`
  (Ignite) — the structural `db-column-type` variant is intentionally not decoded, matching the
  Scala reference SDK, since no accessor consumes it.
- **Decimal / numeric** values are carried as strings (`PostgresDbValue.Numeric`,
  `MysqlDbValue.Decimal`, `IgniteDbValue.DbDecimal`) to preserve arbitrary precision.
- **Query streaming (`query-stream` / `db-result-stream`) is deliberately not implemented**, in
  line with the Scala SDK. Use `query`, which materialises the full result.

See also: [Transactions](transactions.md) · [Quota](quota.md) · [Types](types.md) ·
[SDK README](../../README.md)
