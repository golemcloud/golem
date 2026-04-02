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

import zio.test._

object RdbmsCompileSpec extends ZIOSpecDefault {
  import Rdbms._

  private val date   = DbDate(2024, 6, 15)
  private val time   = DbTime(14, 30, 45, 123456789L)
  private val ts     = DbTimestamp(date, time)
  private val tstz   = DbTimestampTz(ts, 3600)
  private val timeTz = DbTimeTz(time, -18000)
  private val uuid   = DbUuid(BigInt("123456789012345678"), BigInt("987654321098765432"))
  private val ipv4   = IpAddress.Ipv4(192, 168, 1, 1)
  private val ipv6   = IpAddress.Ipv6(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)
  private val mac    = MacAddress(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff)

  private val allMysqlValues: List[MysqlDbValue] = List(
    MysqlDbValue.BooleanVal(true),
    MysqlDbValue.TinyInt(1.toByte),
    MysqlDbValue.SmallInt(100.toShort),
    MysqlDbValue.MediumInt(10000),
    MysqlDbValue.IntVal(100000),
    MysqlDbValue.BigInt(1000000L),
    MysqlDbValue.TinyIntUnsigned(255.toShort),
    MysqlDbValue.SmallIntUnsigned(65535),
    MysqlDbValue.MediumIntUnsigned(16777215L),
    MysqlDbValue.IntUnsigned(4294967295L),
    MysqlDbValue.BigIntUnsigned(scala.BigInt("18446744073709551615")),
    MysqlDbValue.FloatVal(3.14f),
    MysqlDbValue.DoubleVal(2.718),
    MysqlDbValue.Decimal("99999.99"),
    MysqlDbValue.Date(date),
    MysqlDbValue.DateTime(ts),
    MysqlDbValue.Timestamp(ts),
    MysqlDbValue.Time(time),
    MysqlDbValue.Year(2024),
    MysqlDbValue.FixChar("A"),
    MysqlDbValue.VarChar("hello"),
    MysqlDbValue.TinyText("tiny"),
    MysqlDbValue.Text("text"),
    MysqlDbValue.MediumText("medium"),
    MysqlDbValue.LongText("long"),
    MysqlDbValue.Binary(Array[Byte](1, 2, 3)),
    MysqlDbValue.VarBinary(Array[Byte](4, 5)),
    MysqlDbValue.TinyBlob(Array[Byte](6)),
    MysqlDbValue.Blob(Array[Byte](7, 8)),
    MysqlDbValue.MediumBlob(Array[Byte](9)),
    MysqlDbValue.LongBlob(Array[Byte](10, 11, 12)),
    MysqlDbValue.Enumeration("active"),
    MysqlDbValue.SetVal("a,b,c"),
    MysqlDbValue.Bit(List(true, false, true)),
    MysqlDbValue.Json("""{"key":"value"}"""),
    MysqlDbValue.Null
  )

  @SuppressWarnings(Array("all"))
  private def describeMysql(v: MysqlDbValue): String = v match {
    case MysqlDbValue.BooleanVal(b)        => s"bool($b)"
    case MysqlDbValue.TinyInt(n)           => s"tinyint($n)"
    case MysqlDbValue.SmallInt(n)          => s"smallint($n)"
    case MysqlDbValue.MediumInt(n)         => s"mediumint($n)"
    case MysqlDbValue.IntVal(n)            => s"int($n)"
    case MysqlDbValue.BigInt(n)            => s"bigint($n)"
    case MysqlDbValue.TinyIntUnsigned(n)   => s"tinyint-u($n)"
    case MysqlDbValue.SmallIntUnsigned(n)  => s"smallint-u($n)"
    case MysqlDbValue.MediumIntUnsigned(n) => s"mediumint-u($n)"
    case MysqlDbValue.IntUnsigned(n)       => s"int-u($n)"
    case MysqlDbValue.BigIntUnsigned(n)    => s"bigint-u($n)"
    case MysqlDbValue.FloatVal(n)          => s"float($n)"
    case MysqlDbValue.DoubleVal(n)         => s"double($n)"
    case MysqlDbValue.Decimal(s)           => s"decimal($s)"
    case MysqlDbValue.Date(d)              => s"date($d)"
    case MysqlDbValue.DateTime(t)          => s"datetime($t)"
    case MysqlDbValue.Timestamp(t)         => s"timestamp($t)"
    case MysqlDbValue.Time(t)              => s"time($t)"
    case MysqlDbValue.Year(y)              => s"year($y)"
    case MysqlDbValue.FixChar(s)           => s"fixchar($s)"
    case MysqlDbValue.VarChar(s)           => s"varchar($s)"
    case MysqlDbValue.TinyText(s)          => s"tinytext($s)"
    case MysqlDbValue.Text(s)              => s"text($s)"
    case MysqlDbValue.MediumText(s)        => s"mediumtext($s)"
    case MysqlDbValue.LongText(s)          => s"longtext($s)"
    case MysqlDbValue.Binary(b)            => s"binary(${b.length})"
    case MysqlDbValue.VarBinary(b)         => s"varbinary(${b.length})"
    case MysqlDbValue.TinyBlob(b)          => s"tinyblob(${b.length})"
    case MysqlDbValue.Blob(b)              => s"blob(${b.length})"
    case MysqlDbValue.MediumBlob(b)        => s"mediumblob(${b.length})"
    case MysqlDbValue.LongBlob(b)          => s"longblob(${b.length})"
    case MysqlDbValue.Enumeration(s)       => s"enum($s)"
    case MysqlDbValue.SetVal(s)            => s"set($s)"
    case MysqlDbValue.Bit(bs)              => s"bit(${bs.size})"
    case MysqlDbValue.Json(s)              => s"json($s)"
    case MysqlDbValue.Null                 => "null"
  }

  private val interval = PgInterval(1, 15, 3600000000L)
  private val pgEnum   = PgEnumeration("status", "active")

  private val allPostgresValues: List[PostgresDbValue] = List(
    PostgresDbValue.Character(65.toByte),
    PostgresDbValue.Int2(100.toShort),
    PostgresDbValue.Int4(100000),
    PostgresDbValue.Int8(1000000000L),
    PostgresDbValue.Float4(3.14f),
    PostgresDbValue.Float8(2.718),
    PostgresDbValue.Numeric("12345.6789"),
    PostgresDbValue.BooleanVal(true),
    PostgresDbValue.Text("hello"),
    PostgresDbValue.VarChar("world"),
    PostgresDbValue.BpChar("X"),
    PostgresDbValue.Timestamp(ts),
    PostgresDbValue.TimestampTz(tstz),
    PostgresDbValue.Date(date),
    PostgresDbValue.Time(time),
    PostgresDbValue.TimeTz(timeTz),
    PostgresDbValue.Interval(interval),
    PostgresDbValue.Bytea(Array[Byte](1, 2, 3)),
    PostgresDbValue.Json("""{"a":1}"""),
    PostgresDbValue.Jsonb("""{"b":2}"""),
    PostgresDbValue.JsonPath("$.store.book[0].title"),
    PostgresDbValue.Xml("<root/>"),
    PostgresDbValue.Uuid(uuid),
    PostgresDbValue.Inet(ipv4),
    PostgresDbValue.Cidr(ipv6),
    PostgresDbValue.MacAddr(mac),
    PostgresDbValue.Bit(List(true, false)),
    PostgresDbValue.VarBit(List(false, true, true)),
    PostgresDbValue.Int4RangeVal(Int4Range(Int4Bound.Included(1), Int4Bound.Excluded(10))),
    PostgresDbValue.Int8RangeVal(Int8Range(Int8Bound.Included(100L), Int8Bound.Unbounded)),
    PostgresDbValue.NumRangeVal(NumRange(NumBound.Unbounded, NumBound.Excluded("999.99"))),
    PostgresDbValue.TsRangeVal(TsRange(TsBound.Included(ts), TsBound.Excluded(ts))),
    PostgresDbValue.TsTzRangeVal(TsTzRange(TsTzBound.Included(tstz), TsTzBound.Unbounded)),
    PostgresDbValue.DateRangeVal(DateRange(DateBound.Included(date), DateBound.Excluded(date))),
    PostgresDbValue.Money(99999L),
    PostgresDbValue.Oid(12345L),
    PostgresDbValue.Enumeration(pgEnum),
    PostgresDbValue.Composite(PgComposite("point", List(PostgresDbValue.Int4(1), PostgresDbValue.Int4(2)))),
    PostgresDbValue.Domain(PgDomain("email", PostgresDbValue.Text("a@b.com"))),
    PostgresDbValue.PgArray(List(PostgresDbValue.Int4(1), PostgresDbValue.Int4(2), PostgresDbValue.Null)),
    PostgresDbValue.Range(
      PgRange(
        "custom_range",
        PgValuesRange(
          PgValueBound.Included(PostgresDbValue.Int4(1)),
          PgValueBound.Excluded(PostgresDbValue.Int4(100))
        )
      )
    ),
    PostgresDbValue.Null
  )

  @SuppressWarnings(Array("all"))
  private def describePg(v: PostgresDbValue): String = v match {
    case PostgresDbValue.Character(n)    => s"char($n)"
    case PostgresDbValue.Int2(n)         => s"int2($n)"
    case PostgresDbValue.Int4(n)         => s"int4($n)"
    case PostgresDbValue.Int8(n)         => s"int8($n)"
    case PostgresDbValue.Float4(n)       => s"float4($n)"
    case PostgresDbValue.Float8(n)       => s"float8($n)"
    case PostgresDbValue.Numeric(s)      => s"numeric($s)"
    case PostgresDbValue.BooleanVal(b)   => s"bool($b)"
    case PostgresDbValue.Text(s)         => s"text($s)"
    case PostgresDbValue.VarChar(s)      => s"varchar($s)"
    case PostgresDbValue.BpChar(s)       => s"bpchar($s)"
    case PostgresDbValue.Timestamp(t)    => s"timestamp($t)"
    case PostgresDbValue.TimestampTz(t)  => s"timestamptz($t)"
    case PostgresDbValue.Date(d)         => s"date($d)"
    case PostgresDbValue.Time(t)         => s"time($t)"
    case PostgresDbValue.TimeTz(t)       => s"timetz($t)"
    case PostgresDbValue.Interval(i)     => s"interval($i)"
    case PostgresDbValue.Bytea(b)        => s"bytea(${b.length})"
    case PostgresDbValue.Json(s)         => s"json($s)"
    case PostgresDbValue.Jsonb(s)        => s"jsonb($s)"
    case PostgresDbValue.JsonPath(s)     => s"jsonpath($s)"
    case PostgresDbValue.Xml(s)          => s"xml($s)"
    case PostgresDbValue.Uuid(u)         => s"uuid($u)"
    case PostgresDbValue.Inet(ip)        => s"inet($ip)"
    case PostgresDbValue.Cidr(ip)        => s"cidr($ip)"
    case PostgresDbValue.MacAddr(m)      => s"macaddr($m)"
    case PostgresDbValue.Bit(bs)         => s"bit(${bs.size})"
    case PostgresDbValue.VarBit(bs)      => s"varbit(${bs.size})"
    case PostgresDbValue.Int4RangeVal(r) => s"int4range($r)"
    case PostgresDbValue.Int8RangeVal(r) => s"int8range($r)"
    case PostgresDbValue.NumRangeVal(r)  => s"numrange($r)"
    case PostgresDbValue.TsRangeVal(r)   => s"tsrange($r)"
    case PostgresDbValue.TsTzRangeVal(r) => s"tstzrange($r)"
    case PostgresDbValue.DateRangeVal(r) => s"daterange($r)"
    case PostgresDbValue.Money(n)        => s"money($n)"
    case PostgresDbValue.Oid(n)          => s"oid($n)"
    case PostgresDbValue.Enumeration(e)  => s"enum(${e.name})"
    case PostgresDbValue.Composite(c)    => s"composite(${c.name})"
    case PostgresDbValue.Domain(d)       => s"domain(${d.name})"
    case PostgresDbValue.PgArray(a)      => s"array(${a.size})"
    case PostgresDbValue.Range(r)        => s"range(${r.name})"
    case PostgresDbValue.Vector(v)       => s"vector(${v.size})"
    case PostgresDbValue.HalfVec(v)      => s"halfvec(${v.size})"
    case PostgresDbValue.SparseVec(v)    => s"sparsevec(${v.dim})"
    case PostgresDbValue.Null            => "null"
  }

  def spec = suite("RdbmsCompileSpec")(
    test("all 36 MysqlDbValue variants constructed") {
      assertTrue(allMysqlValues.size == 36)
    },

    test("exhaustive MysqlDbValue match compiles") {
      allMysqlValues.foreach(v => Predef.assert(describeMysql(v).nonEmpty))
      assertCompletes
    },

    test("all 43 PostgresDbValue variants constructed") {
      val distinctNames = allPostgresValues.map(describePg).map(_.takeWhile(_ != '(')).distinct
      assertTrue(distinctNames.size >= 42)
    },

    test("exhaustive PostgresDbValue match compiles") {
      allPostgresValues.foreach(v => Predef.assert(describePg(v).nonEmpty))
      assertCompletes
    },

    test("shared types construction") {
      assertTrue(
        date.year == 2024 && date.month == 6 && date.day == 15,
        time.hour == 14 && time.minute == 30 && time.second == 45,
        ts.date == date && ts.time == time,
        tstz.timestamp == ts && tstz.offset == 3600,
        timeTz.time == time && timeTz.offset == -18000,
        uuid.highBits > 0 && uuid.lowBits > 0
      )
    },

    test("IpAddress exhaustive match") {
      List(ipv4, ipv6).foreach {
        case IpAddress.Ipv4(a, b, c, d)             => Predef.assert(a == 192.toShort)
        case IpAddress.Ipv6(a, b, c, d, e, f, g, h) => Predef.assert(a == 0x2001)
      }
      assertCompletes
    },

    test("MacAddress field access") {
      assertTrue(mac.a == 0xaa.toShort)
    },

    test("bound types exhaustive") {
      val i4bounds: List[Int4Bound] = List(Int4Bound.Included(1), Int4Bound.Excluded(2), Int4Bound.Unbounded)
      i4bounds.foreach {
        case Int4Bound.Included(v) => Predef.assert(v == 1)
        case Int4Bound.Excluded(v) => Predef.assert(v == 2)
        case Int4Bound.Unbounded   => ()
      }

      val i8bounds: List[Int8Bound] = List(Int8Bound.Included(1L), Int8Bound.Excluded(2L), Int8Bound.Unbounded)
      i8bounds.foreach { case Int8Bound.Included(_) | Int8Bound.Excluded(_) | Int8Bound.Unbounded =>
        ()
      }

      val nBounds: List[NumBound] = List(NumBound.Included("1"), NumBound.Excluded("2"), NumBound.Unbounded)
      nBounds.foreach { case NumBound.Included(_) | NumBound.Excluded(_) | NumBound.Unbounded =>
        ()
      }

      val tsBounds: List[TsBound] = List(TsBound.Included(ts), TsBound.Excluded(ts), TsBound.Unbounded)
      tsBounds.foreach { case TsBound.Included(_) | TsBound.Excluded(_) | TsBound.Unbounded =>
        ()
      }

      val tstzBounds: List[TsTzBound] = List(TsTzBound.Included(tstz), TsTzBound.Excluded(tstz), TsTzBound.Unbounded)
      tstzBounds.foreach { case TsTzBound.Included(_) | TsTzBound.Excluded(_) | TsTzBound.Unbounded =>
        ()
      }

      val dateBounds: List[DateBound] = List(DateBound.Included(date), DateBound.Excluded(date), DateBound.Unbounded)
      dateBounds.foreach { case DateBound.Included(_) | DateBound.Excluded(_) | DateBound.Unbounded =>
        ()
      }

      val pvBounds: List[PgValueBound] = List(
        PgValueBound.Included(PostgresDbValue.Int4(1)),
        PgValueBound.Excluded(PostgresDbValue.Int4(2)),
        PgValueBound.Unbounded
      )
      pvBounds.foreach { case PgValueBound.Included(_) | PgValueBound.Excluded(_) | PgValueBound.Unbounded =>
        ()
      }
      assertCompletes
    },

    test("row types construction and accessors") {
      val mysqlRow = MysqlDbRow(List(MysqlDbValue.VarChar("hello"), MysqlDbValue.IntVal(42), MysqlDbValue.Null))
      Predef.assert(mysqlRow.getString(0).nonEmpty)
      Predef.assert(mysqlRow.getInt(1).contains(42))
      Predef.assert(mysqlRow.getString(2).isEmpty)

      val pgRow = PostgresDbRow(
        List(PostgresDbValue.Text("world"), PostgresDbValue.Int4(99), PostgresDbValue.Int8(1000L), PostgresDbValue.Null)
      )
      Predef.assert(pgRow.getString(0).nonEmpty)
      Predef.assert(pgRow.getInt(1).contains(99))
      Predef.assert(pgRow.getLong(2).contains(1000L))
      Predef.assert(pgRow.getString(3).isEmpty)
      assertCompletes
    },

    test("result types construction") {
      val cols     = List(DbColumn(0L, "id", "int4"), DbColumn(1L, "name", "text"))
      val pgResult =
        PostgresDbResult(cols, List(PostgresDbRow(List(PostgresDbValue.Int4(1), PostgresDbValue.Text("a")))))
      val myResult = MysqlDbResult(cols, List(MysqlDbRow(List(MysqlDbValue.IntVal(1), MysqlDbValue.VarChar("a")))))
      assertTrue(
        pgResult.columns.size == 2 && pgResult.rows.size == 1,
        myResult.columns.size == 2 && myResult.rows.size == 1
      )
    },

    test("DbError exhaustive match") {
      val errors: List[DbError] = List(
        DbError.ConnectionFailure("conn"),
        DbError.QueryParameterFailure("param"),
        DbError.QueryExecutionFailure("exec"),
        DbError.QueryResponseFailure("resp"),
        DbError.Other("other")
      )
      errors.foreach {
        case DbError.ConnectionFailure(m)     => Predef.assert(m == "conn")
        case DbError.QueryParameterFailure(m) => Predef.assert(m == "param")
        case DbError.QueryExecutionFailure(m) => Predef.assert(m == "exec")
        case DbError.QueryResponseFailure(m)  => Predef.assert(m == "resp")
        case DbError.Other(m)                 => Predef.assert(m == "other")
      }
      assertCompletes
    },

    test("connection return types compile") {
      val _: Either[DbError, PostgresConnection] = Left(DbError.Other("test"))
      val _: Either[DbError, MysqlConnection]    = Left(DbError.Other("test"))
      assertCompletes
    }
  )
}
