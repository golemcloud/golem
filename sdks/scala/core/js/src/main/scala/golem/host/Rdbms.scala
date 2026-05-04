/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host

import golem.host.js._

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Scala.js facades for the Golem RDBMS host packages.
 *
 * Provides fully typed wrappers for `golem:rdbms/postgres@1.5.0`,
 * `golem:rdbms/mysql@1.5.0`, and shared types from `golem:rdbms/types@1.5.0`.
 */
object Rdbms {

  // ===========================================================================
  // Shared types (golem:rdbms/types@1.5.0)
  // ===========================================================================

  final case class DbDate(year: Int, month: Short, day: Short)

  final case class DbTime(hour: Short, minute: Short, second: Short, nanosecond: Long)

  final case class DbTimestamp(date: DbDate, time: DbTime)

  final case class DbTimestampTz(timestamp: DbTimestamp, offset: Int)

  final case class DbTimeTz(time: DbTime, offset: Int)

  final case class DbUuid(highBits: BigInt, lowBits: BigInt)

  sealed trait IpAddress extends Product with Serializable
  object IpAddress {
    final case class Ipv4(a: Short, b: Short, c: Short, d: Short)                         extends IpAddress
    final case class Ipv6(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int) extends IpAddress
  }

  final case class MacAddress(a: Short, b: Short, c: Short, d: Short, e: Short, f: Short)

  // Parsing helpers for shared types

  private def parseDbDate(raw: JsDbDate): DbDate =
    DbDate(raw.year, raw.month.toShort, raw.day.toShort)

  private def parseDbTime(raw: JsDbTime): DbTime =
    DbTime(
      raw.hour.toShort,
      raw.minute.toShort,
      raw.second.toShort,
      raw.nanosecond.toLong
    )

  private def parseDbTimestamp(raw: JsDbTimestamp): DbTimestamp =
    DbTimestamp(parseDbDate(raw.date), parseDbTime(raw.time))

  private def parseDbTimestampTz(raw: JsDbTimestampTz): DbTimestampTz =
    DbTimestampTz(parseDbTimestamp(raw.timestamp), raw.offset)

  private def parseDbTimeTz(raw: JsDbTimeTz): DbTimeTz =
    DbTimeTz(parseDbTime(raw.time), raw.offset)

  private def parseDbUuid(raw: JsDbUuid): DbUuid =
    DbUuid(BigInt(raw.highBits.toString), BigInt(raw.lowBits.toString))

  private def parseIpAddress(raw: JsIpAddress): IpAddress =
    raw.tag match {
      case "ipv4" =>
        val t = raw.asInstanceOf[JsIpAddressIpv4].value
        IpAddress.Ipv4(t._1.toShort, t._2.toShort, t._3.toShort, t._4.toShort)
      case "ipv6" =>
        val arr = raw.asInstanceOf[JsIpAddressIpv6].value
        IpAddress.Ipv6(arr(0), arr(1), arr(2), arr(3), arr(4), arr(5), arr(6), arr(7))
      case other =>
        throw new IllegalArgumentException(s"Unknown IpAddress tag: $other")
    }

  private def parseMacAddress(raw: JsMacAddress): MacAddress = {
    val o = raw.octets
    MacAddress(o(0).toShort, o(1).toShort, o(2).toShort, o(3).toShort, o(4).toShort, o(5).toShort)
  }

  // toJs helpers for shared types

  private def dbDateToJs(d: DbDate): JsDbDate =
    JsDbDate(d.year, d.month.toInt, d.day.toInt)

  private def dbTimeToJs(t: DbTime): JsDbTime =
    JsDbTime(t.hour.toInt, t.minute.toInt, t.second.toInt, t.nanosecond.toInt)

  private def dbTimestampToJs(ts: DbTimestamp): JsDbTimestamp =
    JsDbTimestamp(dbDateToJs(ts.date), dbTimeToJs(ts.time))

  private def dbTimestampTzToJs(tstz: DbTimestampTz): JsDbTimestampTz =
    JsDbTimestampTz(dbTimestampToJs(tstz.timestamp), tstz.offset)

  private def dbTimeTzToJs(ttz: DbTimeTz): JsDbTimeTz =
    JsDbTimeTz(dbTimeToJs(ttz.time), ttz.offset)

  private def dbUuidToJs(u: DbUuid): JsDbUuid =
    JsDbUuid(js.BigInt(u.highBits.toString), js.BigInt(u.lowBits.toString))

  private def ipAddressToJs(ip: IpAddress): JsIpAddress = ip match {
    case IpAddress.Ipv4(a, b, c, d)             => JsIpAddress.ipv4(a.toInt, b.toInt, c.toInt, d.toInt)
    case IpAddress.Ipv6(a, b, c, d, e, f, g, h) =>
      JsIpAddress.ipv6(js.Array(a, b, c, d, e, f, g, h))
  }

  private def macAddressToJs(m: MacAddress): JsMacAddress =
    JsMacAddress(js.Array(m.a.toInt, m.b.toInt, m.c.toInt, m.d.toInt, m.e.toInt, m.f.toInt))

  // ===========================================================================
  // MySQL db-value (36 variants)
  // ===========================================================================

  sealed trait MysqlDbValue extends Product with Serializable
  object MysqlDbValue {
    final case class BooleanVal(value: Boolean)          extends MysqlDbValue
    final case class TinyInt(value: Byte)                extends MysqlDbValue
    final case class SmallInt(value: Short)              extends MysqlDbValue
    final case class MediumInt(value: Int)               extends MysqlDbValue
    final case class IntVal(value: Int)                  extends MysqlDbValue
    final case class BigInt(value: Long)                 extends MysqlDbValue
    final case class TinyIntUnsigned(value: Short)       extends MysqlDbValue
    final case class SmallIntUnsigned(value: Int)        extends MysqlDbValue
    final case class MediumIntUnsigned(value: Long)      extends MysqlDbValue
    final case class IntUnsigned(value: Long)            extends MysqlDbValue
    final case class BigIntUnsigned(value: scala.BigInt) extends MysqlDbValue
    final case class FloatVal(value: Float)              extends MysqlDbValue
    final case class DoubleVal(value: Double)            extends MysqlDbValue
    final case class Decimal(value: String)              extends MysqlDbValue
    final case class Date(value: DbDate)                 extends MysqlDbValue
    final case class DateTime(value: DbTimestamp)        extends MysqlDbValue
    final case class Timestamp(value: DbTimestamp)       extends MysqlDbValue
    final case class Time(value: DbTime)                 extends MysqlDbValue
    final case class Year(value: Int)                    extends MysqlDbValue
    final case class FixChar(value: String)              extends MysqlDbValue
    final case class VarChar(value: String)              extends MysqlDbValue
    final case class TinyText(value: String)             extends MysqlDbValue
    final case class Text(value: String)                 extends MysqlDbValue
    final case class MediumText(value: String)           extends MysqlDbValue
    final case class LongText(value: String)             extends MysqlDbValue
    final case class Binary(value: Array[Byte])          extends MysqlDbValue
    final case class VarBinary(value: Array[Byte])       extends MysqlDbValue
    final case class TinyBlob(value: Array[Byte])        extends MysqlDbValue
    final case class Blob(value: Array[Byte])            extends MysqlDbValue
    final case class MediumBlob(value: Array[Byte])      extends MysqlDbValue
    final case class LongBlob(value: Array[Byte])        extends MysqlDbValue
    final case class Enumeration(value: String)          extends MysqlDbValue
    final case class SetVal(value: String)               extends MysqlDbValue
    final case class Bit(value: List[Boolean])           extends MysqlDbValue
    final case class Json(value: String)                 extends MysqlDbValue
    case object Null                                     extends MysqlDbValue

    def fromJs(raw: JsMysqlDbValue): MysqlDbValue =
      raw.tag match {
        case "null" => Null
        case _      =>
          val v = raw.asInstanceOf[JsMysqlDbValueWithValue].value
          raw.tag match {
            case "boolean"            => BooleanVal(v.asInstanceOf[Boolean])
            case "tinyint"            => TinyInt(v.asInstanceOf[Double].toByte)
            case "smallint"           => SmallInt(v.asInstanceOf[Double].toShort)
            case "mediumint"          => MediumInt(v.asInstanceOf[Double].toInt)
            case "int"                => IntVal(v.asInstanceOf[Double].toInt)
            case "bigint"             => BigInt(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "tinyint-unsigned"   => TinyIntUnsigned(v.asInstanceOf[Double].toShort)
            case "smallint-unsigned"  => SmallIntUnsigned(v.asInstanceOf[Double].toInt)
            case "mediumint-unsigned" => MediumIntUnsigned(jsAnyToLong(v))
            case "int-unsigned"       => IntUnsigned(jsAnyToLong(v))
            case "bigint-unsigned"    => BigIntUnsigned(scala.BigInt(v.asInstanceOf[js.BigInt].toString))
            case "float"              => FloatVal(v.asInstanceOf[Double].toFloat)
            case "double"             => DoubleVal(v.asInstanceOf[Double])
            case "decimal"            => Decimal(v.asInstanceOf[String])
            case "date"               => Date(parseDbDate(v.asInstanceOf[JsDbDate]))
            case "datetime"           => DateTime(parseDbTimestamp(v.asInstanceOf[JsDbTimestamp]))
            case "timestamp"          => Timestamp(parseDbTimestamp(v.asInstanceOf[JsDbTimestamp]))
            case "time"               => Time(parseDbTime(v.asInstanceOf[JsDbTime]))
            case "year"               => Year(v.asInstanceOf[Double].toInt)
            case "fixchar"            => FixChar(v.asInstanceOf[String])
            case "varchar"            => VarChar(v.asInstanceOf[String])
            case "tinytext"           => TinyText(v.asInstanceOf[String])
            case "text"               => Text(v.asInstanceOf[String])
            case "mediumtext"         => MediumText(v.asInstanceOf[String])
            case "longtext"           => LongText(v.asInstanceOf[String])
            case "binary"             => Binary(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "varbinary"          => VarBinary(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "tinyblob"           => TinyBlob(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "blob"               => Blob(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "mediumblob"         => MediumBlob(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "longblob"           => LongBlob(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "enumeration"        => Enumeration(v.asInstanceOf[String])
            case "set"                => SetVal(v.asInstanceOf[String])
            case "bit"                => Bit(v.asInstanceOf[js.Array[Boolean]].toList)
            case "json"               => Json(v.asInstanceOf[String])
            case other                => throw new IllegalArgumentException(s"Unknown MySQL db-value tag: $other")
          }
      }

    def fromDynamic(raw: js.Dynamic): MysqlDbValue = fromJs(raw.asInstanceOf[JsMysqlDbValue])

    def toJs(v: MysqlDbValue): JsMysqlDbValue = v match {
      case Null                 => JsMysqlDbValue.`null`
      case BooleanVal(b)        => JsMysqlDbValue.boolean(b)
      case TinyInt(n)           => JsMysqlDbValue.tinyint(n.toInt)
      case SmallInt(n)          => JsMysqlDbValue.smallint(n.toInt)
      case MediumInt(n)         => JsMysqlDbValue.mediumint(n)
      case IntVal(n)            => JsMysqlDbValue.int(n)
      case BigInt(n)            => JsMysqlDbValue.bigint(js.BigInt(n.toString))
      case TinyIntUnsigned(n)   => JsMysqlDbValue.tinyintUnsigned(n.toInt)
      case SmallIntUnsigned(n)  => JsMysqlDbValue.smallintUnsigned(n)
      case MediumIntUnsigned(n) => JsMysqlDbValue.mediumintUnsigned(n.toInt)
      case IntUnsigned(n)       => JsMysqlDbValue.intUnsigned(n.toDouble)
      case BigIntUnsigned(n)    => JsMysqlDbValue.bigintUnsigned(js.BigInt(n.toString))
      case FloatVal(n)          => JsMysqlDbValue.float(n.toDouble)
      case DoubleVal(n)         => JsMysqlDbValue.double(n)
      case Decimal(s)           => JsMysqlDbValue.decimal(s)
      case Date(d)              => JsMysqlDbValue.date(dbDateToJs(d))
      case DateTime(ts)         => JsMysqlDbValue.datetime(dbTimestampToJs(ts))
      case Timestamp(ts)        => JsMysqlDbValue.timestamp(dbTimestampToJs(ts))
      case Time(t)              => JsMysqlDbValue.time(dbTimeToJs(t))
      case Year(y)              => JsMysqlDbValue.year(y)
      case FixChar(s)           => JsMysqlDbValue.fixchar(s)
      case VarChar(s)           => JsMysqlDbValue.varchar(s)
      case TinyText(s)          => JsMysqlDbValue.tinytext(s)
      case Text(s)              => JsMysqlDbValue.text(s)
      case MediumText(s)        => JsMysqlDbValue.mediumtext(s)
      case LongText(s)          => JsMysqlDbValue.longtext(s)
      case Binary(b)            => JsMysqlDbValue.binary(bytesToUint8Array(b))
      case VarBinary(b)         => JsMysqlDbValue.varbinary(bytesToUint8Array(b))
      case TinyBlob(b)          => JsMysqlDbValue.tinyblob(bytesToUint8Array(b))
      case Blob(b)              => JsMysqlDbValue.blob(bytesToUint8Array(b))
      case MediumBlob(b)        => JsMysqlDbValue.mediumblob(bytesToUint8Array(b))
      case LongBlob(b)          => JsMysqlDbValue.longblob(bytesToUint8Array(b))
      case Enumeration(s)       => JsMysqlDbValue.enumeration(s)
      case SetVal(s)            => JsMysqlDbValue.set(s)
      case Bit(bs)              => JsMysqlDbValue.bit(js.Array(bs: _*))
      case Json(s)              => JsMysqlDbValue.json(s)
    }

    def toDynamic(v: MysqlDbValue): js.Dynamic = toJs(v).asInstanceOf[js.Dynamic]
  }

  // ===========================================================================
  // Ignite db-value (17 variants including null)
  // ===========================================================================

  sealed trait IgniteDbValue extends Product with Serializable
  object IgniteDbValue {
    case object DbNull                                             extends IgniteDbValue
    final case class DbBoolean(value: Boolean)                     extends IgniteDbValue
    final case class DbByte(value: Byte)                           extends IgniteDbValue
    final case class DbShort(value: Short)                         extends IgniteDbValue
    final case class DbInt(value: Int)                             extends IgniteDbValue
    final case class DbLong(value: Long)                           extends IgniteDbValue
    final case class DbFloat(value: Float)                         extends IgniteDbValue
    final case class DbDouble(value: Double)                       extends IgniteDbValue
    final case class DbChar(value: Char)                           extends IgniteDbValue
    final case class DbString(value: String)                       extends IgniteDbValue
    final case class DbUuid(value: (scala.BigInt, scala.BigInt))   extends IgniteDbValue
    final case class DbDate(value: Long)                           extends IgniteDbValue
    final case class DbTimestamp(value: (Long, Int))               extends IgniteDbValue
    final case class DbTime(value: Long)                           extends IgniteDbValue
    final case class DbDecimal(value: String)                      extends IgniteDbValue
    final case class DbByteArray(value: Array[Byte])               extends IgniteDbValue

    def fromJs(raw: JsIgniteDbValue): IgniteDbValue =
      raw.tag match {
        case "db-null" => DbNull
        case _ =>
          val v = raw.asInstanceOf[JsIgniteDbValueWithValue].value
          raw.tag match {
            case "db-boolean"    => DbBoolean(v.asInstanceOf[Boolean])
            case "db-byte"       => DbByte(v.asInstanceOf[Double].toByte)
            case "db-short"      => DbShort(v.asInstanceOf[Double].toShort)
            case "db-int"        => DbInt(v.asInstanceOf[Double].toInt)
            case "db-long"       => DbLong(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "db-float"      => DbFloat(v.asInstanceOf[Double].toFloat)
            case "db-double"     => DbDouble(v.asInstanceOf[Double])
            case "db-char"       => DbChar(v.asInstanceOf[Double].toChar)
            case "db-string"     => DbString(v.asInstanceOf[String])
            case "db-uuid"       =>
              val t = v.asInstanceOf[js.Tuple2[js.BigInt, js.BigInt]]
              DbUuid((scala.BigInt(t._1.toString), scala.BigInt(t._2.toString)))
            case "db-date"       => DbDate(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "db-timestamp"  =>
              val t = v.asInstanceOf[js.Tuple2[js.BigInt, Double]]
              DbTimestamp((scala.BigInt(t._1.toString).toLong, t._2.toInt))
            case "db-time"       => DbTime(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "db-decimal"    => DbDecimal(v.asInstanceOf[String])
            case "db-byte-array" => DbByteArray(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case other           => throw new IllegalArgumentException(s"Unknown Ignite db-value tag: $other")
          }
      }

    def fromDynamic(raw: js.Dynamic): IgniteDbValue = fromJs(raw.asInstanceOf[JsIgniteDbValue])

    def toJs(v: IgniteDbValue): JsIgniteDbValue = v match {
      case DbNull        => JsIgniteDbValue.dbNull
      case DbBoolean(b)  => JsIgniteDbValue.dbBoolean(b)
      case DbByte(n)     => JsIgniteDbValue.dbByte(n.toInt)
      case DbShort(n)    => JsIgniteDbValue.dbShort(n.toInt)
      case DbInt(n)      => JsIgniteDbValue.dbInt(n)
      case DbLong(n)     => JsIgniteDbValue.dbLong(js.BigInt(n.toString))
      case DbFloat(n)    => JsIgniteDbValue.dbFloat(n.toDouble)
      case DbDouble(n)   => JsIgniteDbValue.dbDouble(n)
      case DbChar(c)     => JsIgniteDbValue.dbChar(c.toInt)
      case DbString(s)   => JsIgniteDbValue.dbString(s)
      case DbUuid(t)     => JsIgniteDbValue.dbUuid(js.Tuple2(js.BigInt(t._1.toString), js.BigInt(t._2.toString)))
      case DbDate(ms)    => JsIgniteDbValue.dbDate(js.BigInt(ms.toString))
      case DbTimestamp(t) => JsIgniteDbValue.dbTimestamp(js.Tuple2(js.BigInt(t._1.toString), t._2))
      case DbTime(ns)    => JsIgniteDbValue.dbTime(js.BigInt(ns.toString))
      case DbDecimal(s)  => JsIgniteDbValue.dbDecimal(s)
      case DbByteArray(b) => JsIgniteDbValue.dbByteArray(bytesToUint8Array(b))
    }

    def toDynamic(v: IgniteDbValue): js.Dynamic = toJs(v).asInstanceOf[js.Dynamic]
  }

  // ===========================================================================
  // Postgres supporting types (ranges, composites, etc.)
  // ===========================================================================

  final case class PgInterval(months: Int, days: Int, microseconds: Long)

  sealed trait Int4Bound extends Product with Serializable
  object Int4Bound {
    final case class Included(value: Int) extends Int4Bound
    final case class Excluded(value: Int) extends Int4Bound
    case object Unbounded                 extends Int4Bound
  }

  final case class Int4Range(start: Int4Bound, end: Int4Bound)

  sealed trait Int8Bound extends Product with Serializable
  object Int8Bound {
    final case class Included(value: Long) extends Int8Bound
    final case class Excluded(value: Long) extends Int8Bound
    case object Unbounded                  extends Int8Bound
  }

  final case class Int8Range(start: Int8Bound, end: Int8Bound)

  sealed trait NumBound extends Product with Serializable
  object NumBound {
    final case class Included(value: String) extends NumBound
    final case class Excluded(value: String) extends NumBound
    case object Unbounded                    extends NumBound
  }

  final case class NumRange(start: NumBound, end: NumBound)

  sealed trait TsBound extends Product with Serializable
  object TsBound {
    final case class Included(value: DbTimestamp) extends TsBound
    final case class Excluded(value: DbTimestamp) extends TsBound
    case object Unbounded                         extends TsBound
  }

  final case class TsRange(start: TsBound, end: TsBound)

  sealed trait TsTzBound extends Product with Serializable
  object TsTzBound {
    final case class Included(value: DbTimestampTz) extends TsTzBound
    final case class Excluded(value: DbTimestampTz) extends TsTzBound
    case object Unbounded                           extends TsTzBound
  }

  final case class TsTzRange(start: TsTzBound, end: TsTzBound)

  sealed trait DateBound extends Product with Serializable
  object DateBound {
    final case class Included(value: DbDate) extends DateBound
    final case class Excluded(value: DbDate) extends DateBound
    case object Unbounded                    extends DateBound
  }

  final case class DateRange(start: DateBound, end: DateBound)

  final case class PgEnumeration(name: String, value: String)

  final case class PgComposite(name: String, values: List[PostgresDbValue])

  final case class PgDomain(name: String, value: PostgresDbValue)

  sealed trait PgValueBound extends Product with Serializable
  object PgValueBound {
    final case class Included(value: PostgresDbValue) extends PgValueBound
    final case class Excluded(value: PostgresDbValue) extends PgValueBound
    case object Unbounded                             extends PgValueBound
  }

  final case class PgValuesRange(start: PgValueBound, end: PgValueBound)

  final case class PgRange(name: String, value: PgValuesRange)

  // Parsing helpers for Postgres supporting types

  private def parseInt4Bound(raw: JsInt4Bound): Int4Bound =
    raw.tag match {
      case "included"  => Int4Bound.Included(raw.asInstanceOf[JsInt4BoundWithValue].value)
      case "excluded"  => Int4Bound.Excluded(raw.asInstanceOf[JsInt4BoundWithValue].value)
      case "unbounded" => Int4Bound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown Int4Bound tag: $other")
    }

  private def parseInt8Bound(raw: JsInt8Bound): Int8Bound =
    raw.tag match {
      case "included"  => Int8Bound.Included(scala.BigInt(raw.asInstanceOf[JsInt8BoundWithValue].value.toString).toLong)
      case "excluded"  => Int8Bound.Excluded(scala.BigInt(raw.asInstanceOf[JsInt8BoundWithValue].value.toString).toLong)
      case "unbounded" => Int8Bound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown Int8Bound tag: $other")
    }

  private def parseNumBound(raw: JsNumBound): NumBound =
    raw.tag match {
      case "included"  => NumBound.Included(raw.asInstanceOf[JsNumBoundWithValue].value)
      case "excluded"  => NumBound.Excluded(raw.asInstanceOf[JsNumBoundWithValue].value)
      case "unbounded" => NumBound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown NumBound tag: $other")
    }

  private def parseTsBound(raw: JsTsBound): TsBound =
    raw.tag match {
      case "included"  => TsBound.Included(parseDbTimestamp(raw.asInstanceOf[JsTsBoundWithValue].value))
      case "excluded"  => TsBound.Excluded(parseDbTimestamp(raw.asInstanceOf[JsTsBoundWithValue].value))
      case "unbounded" => TsBound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown TsBound tag: $other")
    }

  private def parseTsTzBound(raw: JsTsTzBound): TsTzBound =
    raw.tag match {
      case "included"  => TsTzBound.Included(parseDbTimestampTz(raw.asInstanceOf[JsTsTzBoundWithValue].value))
      case "excluded"  => TsTzBound.Excluded(parseDbTimestampTz(raw.asInstanceOf[JsTsTzBoundWithValue].value))
      case "unbounded" => TsTzBound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown TsTzBound tag: $other")
    }

  private def parseDateBound(raw: JsDateBound): DateBound =
    raw.tag match {
      case "included"  => DateBound.Included(parseDbDate(raw.asInstanceOf[JsDateBoundWithValue].value))
      case "excluded"  => DateBound.Excluded(parseDbDate(raw.asInstanceOf[JsDateBoundWithValue].value))
      case "unbounded" => DateBound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown DateBound tag: $other")
    }

  private def parsePgInterval(raw: JsPgInterval): PgInterval =
    PgInterval(raw.months, raw.days, scala.BigInt(raw.microseconds.toString).toLong)

  private def parsePgEnumeration(raw: JsPgEnumeration): PgEnumeration =
    PgEnumeration(raw.name, raw.value)

  private def parsePgComposite(raw: JsPgComposite): PgComposite = {
    val vals = raw.values.toList.map(lazy_ => PostgresDbValue.fromJs(lazy_.get().asInstanceOf[JsPostgresDbValue]))
    PgComposite(raw.name, vals)
  }

  private def parsePgDomain(raw: JsPgDomain): PgDomain =
    PgDomain(raw.name, PostgresDbValue.fromJs(raw.value.get().asInstanceOf[JsPostgresDbValue]))

  private def parsePgValueBound(raw: JsValueBound): PgValueBound =
    raw.tag match {
      case "included" =>
        PgValueBound.Included(
          PostgresDbValue.fromJs(raw.asInstanceOf[JsValueBoundWithValue].value.get().asInstanceOf[JsPostgresDbValue])
        )
      case "excluded" =>
        PgValueBound.Excluded(
          PostgresDbValue.fromJs(raw.asInstanceOf[JsValueBoundWithValue].value.get().asInstanceOf[JsPostgresDbValue])
        )
      case "unbounded" => PgValueBound.Unbounded
      case other       => throw new IllegalArgumentException(s"Unknown PgValueBound tag: $other")
    }

  // toJs helpers for Postgres supporting types

  private def int4BoundToJs(b: Int4Bound): JsInt4Bound = b match {
    case Int4Bound.Included(v) => JsInt4Bound.included(v)
    case Int4Bound.Excluded(v) => JsInt4Bound.excluded(v)
    case Int4Bound.Unbounded   => JsInt4Bound.unbounded
  }

  private def int4RangeToJs(r: Int4Range): JsInt4Range =
    JsInt4Range(int4BoundToJs(r.start), int4BoundToJs(r.end))

  private def int8BoundToJs(b: Int8Bound): JsInt8Bound = b match {
    case Int8Bound.Included(v) => JsInt8Bound.included(js.BigInt(v.toString))
    case Int8Bound.Excluded(v) => JsInt8Bound.excluded(js.BigInt(v.toString))
    case Int8Bound.Unbounded   => JsInt8Bound.unbounded
  }

  private def int8RangeToJs(r: Int8Range): JsInt8Range =
    JsInt8Range(int8BoundToJs(r.start), int8BoundToJs(r.end))

  private def numBoundToJs(b: NumBound): JsNumBound = b match {
    case NumBound.Included(v) => JsNumBound.included(v)
    case NumBound.Excluded(v) => JsNumBound.excluded(v)
    case NumBound.Unbounded   => JsNumBound.unbounded
  }

  private def numRangeToJs(r: NumRange): JsNumRange =
    JsNumRange(numBoundToJs(r.start), numBoundToJs(r.end))

  private def tsBoundToJs(b: TsBound): JsTsBound = b match {
    case TsBound.Included(v) => JsTsBound.included(dbTimestampToJs(v))
    case TsBound.Excluded(v) => JsTsBound.excluded(dbTimestampToJs(v))
    case TsBound.Unbounded   => JsTsBound.unbounded
  }

  private def tsRangeToJs(r: TsRange): JsTsRange =
    JsTsRange(tsBoundToJs(r.start), tsBoundToJs(r.end))

  private def tsTzBoundToJs(b: TsTzBound): JsTsTzBound = b match {
    case TsTzBound.Included(v) => JsTsTzBound.included(dbTimestampTzToJs(v))
    case TsTzBound.Excluded(v) => JsTsTzBound.excluded(dbTimestampTzToJs(v))
    case TsTzBound.Unbounded   => JsTsTzBound.unbounded
  }

  private def tsTzRangeToJs(r: TsTzRange): JsTsTzRange =
    JsTsTzRange(tsTzBoundToJs(r.start), tsTzBoundToJs(r.end))

  private def dateBoundToJs(b: DateBound): JsDateBound = b match {
    case DateBound.Included(v) => JsDateBound.included(dbDateToJs(v))
    case DateBound.Excluded(v) => JsDateBound.excluded(dbDateToJs(v))
    case DateBound.Unbounded   => JsDateBound.unbounded
  }

  private def dateRangeToJs(r: DateRange): JsDateRange =
    JsDateRange(dateBoundToJs(r.start), dateBoundToJs(r.end))

  // ===========================================================================
  // Postgres db-value (46 variants)
  // ===========================================================================

  final case class PgSparseVec(dim: Int, indices: List[Int], values: List[Double])

  sealed trait PostgresDbValue extends Product with Serializable
  object PostgresDbValue {
    final case class Character(value: Byte)                extends PostgresDbValue
    final case class Int2(value: Short)                    extends PostgresDbValue
    final case class Int4(value: Int)                      extends PostgresDbValue
    final case class Int8(value: Long)                     extends PostgresDbValue
    final case class Float4(value: Float)                  extends PostgresDbValue
    final case class Float8(value: Double)                 extends PostgresDbValue
    final case class Numeric(value: String)                extends PostgresDbValue
    final case class BooleanVal(value: Boolean)            extends PostgresDbValue
    final case class Text(value: String)                   extends PostgresDbValue
    final case class VarChar(value: String)                extends PostgresDbValue
    final case class BpChar(value: String)                 extends PostgresDbValue
    final case class Timestamp(value: DbTimestamp)         extends PostgresDbValue
    final case class TimestampTz(value: DbTimestampTz)     extends PostgresDbValue
    final case class Date(value: DbDate)                   extends PostgresDbValue
    final case class Time(value: DbTime)                   extends PostgresDbValue
    final case class TimeTz(value: DbTimeTz)               extends PostgresDbValue
    final case class Interval(value: PgInterval)           extends PostgresDbValue
    final case class Bytea(value: Array[Byte])             extends PostgresDbValue
    final case class Json(value: String)                   extends PostgresDbValue
    final case class Jsonb(value: String)                  extends PostgresDbValue
    final case class JsonPath(value: String)               extends PostgresDbValue
    final case class Xml(value: String)                    extends PostgresDbValue
    final case class Uuid(value: DbUuid)                   extends PostgresDbValue
    final case class Inet(value: IpAddress)                extends PostgresDbValue
    final case class Cidr(value: IpAddress)                extends PostgresDbValue
    final case class MacAddr(value: MacAddress)            extends PostgresDbValue
    final case class Bit(value: List[Boolean])             extends PostgresDbValue
    final case class VarBit(value: List[Boolean])          extends PostgresDbValue
    final case class Int4RangeVal(value: Int4Range)        extends PostgresDbValue
    final case class Int8RangeVal(value: Int8Range)        extends PostgresDbValue
    final case class NumRangeVal(value: NumRange)          extends PostgresDbValue
    final case class TsRangeVal(value: TsRange)            extends PostgresDbValue
    final case class TsTzRangeVal(value: TsTzRange)        extends PostgresDbValue
    final case class DateRangeVal(value: DateRange)        extends PostgresDbValue
    final case class Money(value: Long)                    extends PostgresDbValue
    final case class Oid(value: Long)                      extends PostgresDbValue
    final case class Enumeration(value: PgEnumeration)     extends PostgresDbValue
    final case class Composite(value: PgComposite)         extends PostgresDbValue
    final case class Domain(value: PgDomain)               extends PostgresDbValue
    final case class PgArray(value: List[PostgresDbValue]) extends PostgresDbValue
    final case class Range(value: PgRange)                 extends PostgresDbValue
    final case class Vector(value: List[Double])           extends PostgresDbValue
    final case class HalfVec(value: List[Double])          extends PostgresDbValue
    final case class SparseVec(value: PgSparseVec)         extends PostgresDbValue
    case object Null                                       extends PostgresDbValue

    def fromJs(raw: JsPostgresDbValue): PostgresDbValue =
      raw.tag match {
        case "null" => Null
        case _      =>
          val v = raw.asInstanceOf[JsPostgresDbValueWithValue].value
          raw.tag match {
            case "character"   => Character(v.asInstanceOf[Double].toByte)
            case "int2"        => Int2(v.asInstanceOf[Double].toShort)
            case "int4"        => Int4(v.asInstanceOf[Double].toInt)
            case "int8"        => Int8(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "float4"      => Float4(v.asInstanceOf[Double].toFloat)
            case "float8"      => Float8(v.asInstanceOf[Double])
            case "numeric"     => Numeric(v.asInstanceOf[String])
            case "boolean"     => BooleanVal(v.asInstanceOf[Boolean])
            case "text"        => Text(v.asInstanceOf[String])
            case "varchar"     => VarChar(v.asInstanceOf[String])
            case "bpchar"      => BpChar(v.asInstanceOf[String])
            case "timestamp"   => Timestamp(parseDbTimestamp(v.asInstanceOf[JsDbTimestamp]))
            case "timestamptz" => TimestampTz(parseDbTimestampTz(v.asInstanceOf[JsDbTimestampTz]))
            case "date"        => Date(parseDbDate(v.asInstanceOf[JsDbDate]))
            case "time"        => Time(parseDbTime(v.asInstanceOf[JsDbTime]))
            case "timetz"      => TimeTz(parseDbTimeTz(v.asInstanceOf[JsDbTimeTz]))
            case "interval"    => Interval(parsePgInterval(v.asInstanceOf[JsPgInterval]))
            case "bytea"       => Bytea(uint8ArrayToBytes(v.asInstanceOf[Uint8Array]))
            case "json"        => Json(v.asInstanceOf[String])
            case "jsonb"       => Jsonb(v.asInstanceOf[String])
            case "jsonpath"    => JsonPath(v.asInstanceOf[String])
            case "xml"         => Xml(v.asInstanceOf[String])
            case "uuid"        => Uuid(parseDbUuid(v.asInstanceOf[JsDbUuid]))
            case "inet"        => Inet(parseIpAddress(v.asInstanceOf[JsIpAddress]))
            case "cidr"        => Cidr(parseIpAddress(v.asInstanceOf[JsIpAddress]))
            case "macaddr"     => MacAddr(parseMacAddress(v.asInstanceOf[JsMacAddress]))
            case "bit"         => Bit(v.asInstanceOf[js.Array[Boolean]].toList)
            case "varbit"      => VarBit(v.asInstanceOf[js.Array[Boolean]].toList)
            case "int4range"   =>
              val r = v.asInstanceOf[JsInt4Range]
              Int4RangeVal(Int4Range(parseInt4Bound(r.start), parseInt4Bound(r.end)))
            case "int8range" =>
              val r = v.asInstanceOf[JsInt8Range]
              Int8RangeVal(Int8Range(parseInt8Bound(r.start), parseInt8Bound(r.end)))
            case "numrange" =>
              val r = v.asInstanceOf[JsNumRange]
              NumRangeVal(NumRange(parseNumBound(r.start), parseNumBound(r.end)))
            case "tsrange" =>
              val r = v.asInstanceOf[JsTsRange]
              TsRangeVal(TsRange(parseTsBound(r.start), parseTsBound(r.end)))
            case "tstzrange" =>
              val r = v.asInstanceOf[JsTsTzRange]
              TsTzRangeVal(TsTzRange(parseTsTzBound(r.start), parseTsTzBound(r.end)))
            case "daterange" =>
              val r = v.asInstanceOf[JsDateRange]
              DateRangeVal(DateRange(parseDateBound(r.start), parseDateBound(r.end)))
            case "money"       => Money(scala.BigInt(v.asInstanceOf[js.BigInt].toString).toLong)
            case "oid"         => Oid(jsAnyToLong(v))
            case "enumeration" => Enumeration(parsePgEnumeration(v.asInstanceOf[JsPgEnumeration]))
            case "composite"   => Composite(parsePgComposite(v.asInstanceOf[JsPgComposite]))
            case "domain"      => Domain(parsePgDomain(v.asInstanceOf[JsPgDomain]))
            case "array"       =>
              PgArray(
                v.asInstanceOf[js.Array[JsLazyDbValue]]
                  .toList
                  .map(lazy_ => fromJs(lazy_.get().asInstanceOf[JsPostgresDbValue]))
              )
            case "range" =>
              val r = v.asInstanceOf[JsPgRange]
              Range(PgRange(r.name, PgValuesRange(parsePgValueBound(r.value.start), parsePgValueBound(r.value.end))))
            case "vector"    => Vector(v.asInstanceOf[js.Array[Double]].toList)
            case "halfvec"   => HalfVec(v.asInstanceOf[js.Array[Double]].toList)
            case "sparsevec" =>
              val sv = v.asInstanceOf[JsPgSparseVec]
              SparseVec(PgSparseVec(sv.dim, sv.indices.toList, sv.values.toList))
            case other => throw new IllegalArgumentException(s"Unknown Postgres db-value tag: $other")
          }
      }

    def fromDynamic(raw: js.Dynamic): PostgresDbValue = fromJs(raw.asInstanceOf[JsPostgresDbValue])

    def toJs(v: PostgresDbValue): JsPostgresDbValue = v match {
      case Null              => JsPostgresDbValue.`null`
      case Character(n)      => JsPostgresDbValue.character(n.toInt)
      case Int2(n)           => JsPostgresDbValue.int2(n.toInt)
      case Int4(n)           => JsPostgresDbValue.int4(n)
      case Int8(n)           => JsPostgresDbValue.int8(js.BigInt(n.toString))
      case Float4(n)         => JsPostgresDbValue.float4(n.toDouble)
      case Float8(n)         => JsPostgresDbValue.float8(n)
      case Numeric(s)        => JsPostgresDbValue.numeric(s)
      case BooleanVal(b)     => JsPostgresDbValue.boolean(b)
      case Text(s)           => JsPostgresDbValue.text(s)
      case VarChar(s)        => JsPostgresDbValue.varchar(s)
      case BpChar(s)         => JsPostgresDbValue.bpchar(s)
      case Timestamp(ts)     => JsPostgresDbValue.timestamp(dbTimestampToJs(ts))
      case TimestampTz(tstz) => JsPostgresDbValue.timestamptz(dbTimestampTzToJs(tstz))
      case Date(d)           => JsPostgresDbValue.date(dbDateToJs(d))
      case Time(t)           => JsPostgresDbValue.time(dbTimeToJs(t))
      case TimeTz(ttz)       => JsPostgresDbValue.timetz(dbTimeTzToJs(ttz))
      case Interval(i)       => JsPostgresDbValue.interval(JsPgInterval(i.months, i.days, js.BigInt(i.microseconds.toString)))
      case Bytea(b)          => JsPostgresDbValue.bytea(bytesToUint8Array(b))
      case Json(s)           => JsPostgresDbValue.json(s)
      case Jsonb(s)          => JsPostgresDbValue.jsonb(s)
      case JsonPath(s)       => JsPostgresDbValue.jsonpath(s)
      case Xml(s)            => JsPostgresDbValue.xml(s)
      case Uuid(u)           => JsPostgresDbValue.uuid(dbUuidToJs(u))
      case Inet(ip)          => JsPostgresDbValue.inet(ipAddressToJs(ip))
      case Cidr(ip)          => JsPostgresDbValue.cidr(ipAddressToJs(ip))
      case MacAddr(m)        => JsPostgresDbValue.macaddr(macAddressToJs(m))
      case Bit(bs)           => JsPostgresDbValue.bit(js.Array(bs: _*))
      case VarBit(bs)        => JsPostgresDbValue.varbit(js.Array(bs: _*))
      case Int4RangeVal(r)   => JsPostgresDbValue.int4range(int4RangeToJs(r))
      case Int8RangeVal(r)   => JsPostgresDbValue.int8range(int8RangeToJs(r))
      case NumRangeVal(r)    => JsPostgresDbValue.numrange(numRangeToJs(r))
      case TsRangeVal(r)     => JsPostgresDbValue.tsrange(tsRangeToJs(r))
      case TsTzRangeVal(r)   => JsPostgresDbValue.tstzrange(tsTzRangeToJs(r))
      case DateRangeVal(r)   => JsPostgresDbValue.daterange(dateRangeToJs(r))
      case Money(n)          => JsPostgresDbValue.money(js.BigInt(n.toString))
      case Oid(n)            => JsPostgresDbValue.oid(n.toInt)
      case Enumeration(e)    => JsPostgresDbValue.enumeration(JsPgEnumeration(e.name, e.value))
      case Vector(v)         => JsPostgresDbValue.vector(js.Array(v: _*))
      case HalfVec(v)        => JsPostgresDbValue.halfvec(js.Array(v: _*))
      case SparseVec(sv)     =>
        JsPostgresDbValue.sparsevec(JsPgSparseVec(sv.dim, js.Array(sv.indices: _*), js.Array(sv.values: _*)))
      case _ => throw new UnsupportedOperationException(s"toJs not yet implemented for: $v")
    }

    def toDynamic(v: PostgresDbValue): js.Dynamic = toJs(v).asInstanceOf[js.Dynamic]
  }

  // ===========================================================================
  // Shared byte-list helpers
  // ===========================================================================

  private def uint8ArrayToBytes(arr: Uint8Array): Array[Byte] =
    new scala.scalajs.js.typedarray.Int8Array(arr.buffer, arr.byteOffset, arr.length).toArray

  private def bytesToUint8Array(bytes: Array[Byte]): Uint8Array = {
    val arr = new Uint8Array(bytes.length)
    var i   = 0
    while (i < bytes.length) {
      arr(i) = (bytes(i) & 0xff).toShort
      i += 1
    }
    arr
  }

  private[golem] def jsAnyToLong(value: js.Any): Long =
    js.typeOf(value) match {
      case "bigint" => scala.BigInt(value.toString).toLong
      case "number" => value.asInstanceOf[Double].toLong
      case other    => throw new IllegalArgumentException(s"Expected number or bigint, got: $other")
    }

  // ===========================================================================
  // Typed row types
  // ===========================================================================

  final case class MysqlDbRow(values: List[MysqlDbValue]) {
    def getString(index: Int): Option[String] = values(index) match {
      case MysqlDbValue.Null => None
      case v                 => Some(v.toString)
    }

    def getInt(index: Int): Option[Int] = values(index) match {
      case MysqlDbValue.Null         => None
      case MysqlDbValue.IntVal(n)    => Some(n)
      case MysqlDbValue.TinyInt(n)   => Some(n.toInt)
      case MysqlDbValue.SmallInt(n)  => Some(n.toInt)
      case MysqlDbValue.MediumInt(n) => Some(n)
      case v                         => Some(v.toString.toInt)
    }
  }

  final case class PostgresDbRow(values: List[PostgresDbValue]) {
    def getString(index: Int): Option[String] = values(index) match {
      case PostgresDbValue.Null => None
      case v                    => Some(v.toString)
    }

    def getInt(index: Int): Option[Int] = values(index) match {
      case PostgresDbValue.Null    => None
      case PostgresDbValue.Int4(n) => Some(n)
      case PostgresDbValue.Int2(n) => Some(n.toInt)
      case v                       => Some(v.toString.toInt)
    }

    def getLong(index: Int): Option[Long] = values(index) match {
      case PostgresDbValue.Null    => None
      case PostgresDbValue.Int8(n) => Some(n)
      case PostgresDbValue.Int4(n) => Some(n.toLong)
      case v                       => Some(v.toString.toLong)
    }
  }

  final case class DbColumn(ordinal: Long, name: String, dbTypeName: String)

  final case class MysqlDbResult(columns: List[DbColumn], rows: List[MysqlDbRow])

  final case class PostgresDbResult(columns: List[DbColumn], rows: List[PostgresDbRow])

  final case class IgniteDbColumn(ordinal: Long, name: String)

  final case class IgniteDbRow(values: List[IgniteDbValue]) {
    def getString(index: Int): Option[String] = values(index) match {
      case IgniteDbValue.DbNull => None
      case v                    => Some(v.toString)
    }

    def getInt(index: Int): Option[Int] = values(index) match {
      case IgniteDbValue.DbNull    => None
      case IgniteDbValue.DbInt(n)  => Some(n)
      case IgniteDbValue.DbByte(n) => Some(n.toInt)
      case IgniteDbValue.DbShort(n) => Some(n.toInt)
      case v                       => Some(v.toString.toInt)
    }

    def getLong(index: Int): Option[Long] = values(index) match {
      case IgniteDbValue.DbNull    => None
      case IgniteDbValue.DbLong(n) => Some(n)
      case IgniteDbValue.DbInt(n)  => Some(n.toLong)
      case v                       => Some(v.toString.toLong)
    }
  }

  final case class IgniteDbResult(columns: List[IgniteDbColumn], rows: List[IgniteDbRow])

  // ===========================================================================
  // Error types
  // ===========================================================================

  sealed trait DbError extends Product with Serializable {
    def message: String
  }

  object DbError {
    final case class ConnectionFailure(message: String)     extends DbError
    final case class QueryParameterFailure(message: String) extends DbError
    final case class QueryExecutionFailure(message: String) extends DbError
    final case class QueryResponseFailure(message: String)  extends DbError
    final case class Other(message: String)                 extends DbError

    private[Rdbms] def fromThrowable(t: Throwable): DbError = {
      val msg = if (t.getMessage != null) t.getMessage else t.toString
      Other(msg)
    }
  }

  // ===========================================================================
  // Native imports
  // ===========================================================================

  @js.native
  @JSImport("golem:rdbms/postgres@1.5.0", "DbConnection")
  private object PostgresDbConnectionClass extends js.Object {
    def open(address: String): JsDbConnection = js.native
  }

  @js.native
  @JSImport("golem:rdbms/postgres@1.5.0", JSImport.Namespace)
  private object PostgresModule extends js.Object

  @js.native
  @JSImport("golem:rdbms/mysql@1.5.0", "DbConnection")
  private object MysqlDbConnectionClass extends js.Object {
    def open(address: String): JsDbConnection = js.native
  }

  @js.native
  @JSImport("golem:rdbms/mysql@1.5.0", JSImport.Namespace)
  private object MysqlModule extends js.Object

  @js.native
  @JSImport("golem:rdbms/ignite2@1.5.0", "DbConnection")
  private object IgniteDbConnectionClass extends js.Object {
    def open(address: String): JsDbConnection = js.native
  }

  @js.native
  @JSImport("golem:rdbms/ignite2@1.5.0", JSImport.Namespace)
  private object IgniteModule extends js.Object

  @js.native
  @JSImport("golem:rdbms/types@1.5.0", JSImport.Namespace)
  private object TypesModule extends js.Object

  // ===========================================================================
  // PostgresConnection resource
  // ===========================================================================

  final class PostgresConnection private[Rdbms] (private val underlying: JsDbConnection) {

    def query(statement: String, params: List[PostgresDbValue] = Nil): Either[DbError, PostgresDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(PostgresDbValue.toJs(p).asInstanceOf[js.Any]))
        val raw = underlying.query(statement, jsParams).asInstanceOf[JsDbResult]
        Right(parsePostgresResult(raw))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[PostgresDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(PostgresDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def beginTransaction(): Either[DbError, PostgresTransaction] =
      try Right(new PostgresTransaction(underlying.beginTransaction()))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  final class PostgresTransaction private[Rdbms] (private val underlying: JsDbTransaction) {

    def query(statement: String, params: List[PostgresDbValue] = Nil): Either[DbError, PostgresDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(PostgresDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(parsePostgresResult(underlying.query(statement, jsParams).asInstanceOf[JsDbResult]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[PostgresDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(PostgresDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def commit(): Either[DbError, Unit] =
      try { underlying.commit(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def rollback(): Either[DbError, Unit] =
      try { underlying.rollback(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  // ===========================================================================
  // MysqlConnection resource
  // ===========================================================================

  final class MysqlConnection private[Rdbms] (private val underlying: JsDbConnection) {

    def query(statement: String, params: List[MysqlDbValue] = Nil): Either[DbError, MysqlDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(MysqlDbValue.toJs(p).asInstanceOf[js.Any]))
        val raw = underlying.query(statement, jsParams).asInstanceOf[JsDbResult]
        Right(parseMysqlResult(raw))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[MysqlDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(MysqlDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def beginTransaction(): Either[DbError, MysqlTransaction] =
      try Right(new MysqlTransaction(underlying.beginTransaction()))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  final class MysqlTransaction private[Rdbms] (private val underlying: JsDbTransaction) {

    def query(statement: String, params: List[MysqlDbValue] = Nil): Either[DbError, MysqlDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(MysqlDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(parseMysqlResult(underlying.query(statement, jsParams).asInstanceOf[JsDbResult]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[MysqlDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(MysqlDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def commit(): Either[DbError, Unit] =
      try { underlying.commit(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def rollback(): Either[DbError, Unit] =
      try { underlying.rollback(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  // ===========================================================================
  // IgniteConnection resource
  // ===========================================================================

  final class IgniteConnection private[Rdbms] (private val underlying: JsDbConnection) {

    def query(statement: String, params: List[IgniteDbValue] = Nil): Either[DbError, IgniteDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(IgniteDbValue.toJs(p).asInstanceOf[js.Any]))
        val raw = underlying.query(statement, jsParams).asInstanceOf[JsDbResult]
        Right(parseIgniteResult(raw))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[IgniteDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(IgniteDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def beginTransaction(): Either[DbError, IgniteTransaction] =
      try Right(new IgniteTransaction(underlying.beginTransaction()))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  final class IgniteTransaction private[Rdbms] (private val underlying: JsDbTransaction) {

    def query(statement: String, params: List[IgniteDbValue] = Nil): Either[DbError, IgniteDbResult] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(IgniteDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(parseIgniteResult(underlying.query(statement, jsParams).asInstanceOf[JsDbResult]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def execute(statement: String, params: List[IgniteDbValue] = Nil): Either[DbError, Long] =
      try {
        val jsParams = js.Array[js.Any]()
        params.foreach(p => jsParams.push(IgniteDbValue.toJs(p).asInstanceOf[js.Any]))
        Right(jsAnyToLong(underlying.execute(statement, jsParams).asInstanceOf[js.Any]))
      } catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def commit(): Either[DbError, Unit] =
      try { underlying.commit(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }

    def rollback(): Either[DbError, Unit] =
      try { underlying.rollback(); Right(()) }
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  // ===========================================================================
  // Result parsing
  // ===========================================================================

  private[golem] def parseColumns(raw: JsDbResult): List[DbColumn] =
    raw.columns.toList.map { c =>
      DbColumn(
        ordinal = jsAnyToLong(c.ordinal),
        name = c.name,
        dbTypeName = c.dbTypeName
      )
    }

  private[golem] def parsePostgresResult(raw: JsDbResult): PostgresDbResult = {
    val cols = parseColumns(raw)
    val rows = raw.rows.toList.map { r =>
      PostgresDbRow(r.values.toList.map(v => PostgresDbValue.fromJs(v.asInstanceOf[JsPostgresDbValue])))
    }
    PostgresDbResult(cols, rows)
  }

  private[golem] def parseMysqlResult(raw: JsDbResult): MysqlDbResult = {
    val cols = parseColumns(raw)
    val rows = raw.rows.toList.map { r =>
      MysqlDbRow(r.values.toList.map(v => MysqlDbValue.fromJs(v.asInstanceOf[JsMysqlDbValue])))
    }
    MysqlDbResult(cols, rows)
  }

  private[golem] def parseIgniteColumns(raw: JsDbResult): List[IgniteDbColumn] =
    raw.columns.toList.map { c =>
      IgniteDbColumn(
        ordinal = jsAnyToLong(c.ordinal),
        name = c.name
      )
    }

  private[golem] def parseIgniteResult(raw: JsDbResult): IgniteDbResult = {
    val cols = parseIgniteColumns(raw)
    val rows = raw.rows.toList.map { r =>
      IgniteDbRow(r.values.toList.map(v => IgniteDbValue.fromJs(v.asInstanceOf[JsIgniteDbValue])))
    }
    IgniteDbResult(cols, rows)
  }

  // ===========================================================================
  // Top-level factory methods
  // ===========================================================================

  object Postgres {
    def open(address: String): Either[DbError, PostgresConnection] =
      try Right(new PostgresConnection(PostgresDbConnectionClass.open(address)))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  object Mysql {
    def open(address: String): Either[DbError, MysqlConnection] =
      try Right(new MysqlConnection(MysqlDbConnectionClass.open(address)))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  object Ignite {
    def open(address: String): Either[DbError, IgniteConnection] =
      try Right(new IgniteConnection(IgniteDbConnectionClass.open(address)))
      catch { case t: Throwable => Left(DbError.fromThrowable(t)) }
  }

  def postgresRaw: Any = PostgresModule
  def mysqlRaw: Any    = MysqlModule
  def igniteRaw: Any   = IgniteModule
  def typesRaw: Any    = TypesModule
}
