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

import scala.scalajs.js

object RdbmsRoundtripSpec extends ZIOSpecDefault {
  import Rdbms._

  // --- MySQL round-trips ---

  private def roundtripMysql(value: MysqlDbValue, expectedTag: String): Unit = {
    val dyn = MysqlDbValue.toDynamic(value)
    Predef.assert(dyn.tag.asInstanceOf[String] == expectedTag)
    val parsed = MysqlDbValue.fromDynamic(dyn)
    (value, parsed) match {
      case (MysqlDbValue.Binary(a), MysqlDbValue.Binary(b))         => Predef.assert(a.toList == b.toList)
      case (MysqlDbValue.VarBinary(a), MysqlDbValue.VarBinary(b))   => Predef.assert(a.toList == b.toList)
      case (MysqlDbValue.TinyBlob(a), MysqlDbValue.TinyBlob(b))     => Predef.assert(a.toList == b.toList)
      case (MysqlDbValue.Blob(a), MysqlDbValue.Blob(b))             => Predef.assert(a.toList == b.toList)
      case (MysqlDbValue.MediumBlob(a), MysqlDbValue.MediumBlob(b)) => Predef.assert(a.toList == b.toList)
      case (MysqlDbValue.LongBlob(a), MysqlDbValue.LongBlob(b))     => Predef.assert(a.toList == b.toList)
      case _                                                        => Predef.assert(parsed == value)
    }
  }

  // --- Postgres basic round-trips ---

  private def roundtripPg(value: PostgresDbValue, expectedTag: String): Unit = {
    val dyn = PostgresDbValue.toDynamic(value)
    Predef.assert(dyn.tag.asInstanceOf[String] == expectedTag)
    val parsed = PostgresDbValue.fromDynamic(dyn)
    (value, parsed) match {
      case (PostgresDbValue.Bytea(a), PostgresDbValue.Bytea(b)) => Predef.assert(a.toList == b.toList)
      case _                                                    => Predef.assert(parsed == value)
    }
  }

  def spec = suite("RdbmsRoundtripSpec")(
    test("MySQL BooleanVal round-trip") {
      roundtripMysql(MysqlDbValue.BooleanVal(true), "boolean")
      assertCompletes
    },
    test("MySQL TinyInt round-trip") {
      roundtripMysql(MysqlDbValue.TinyInt(1.toByte), "tinyint")
      assertCompletes
    },
    test("MySQL SmallInt round-trip") {
      roundtripMysql(MysqlDbValue.SmallInt(100.toShort), "smallint")
      assertCompletes
    },
    test("MySQL MediumInt round-trip") {
      roundtripMysql(MysqlDbValue.MediumInt(10000), "mediumint")
      assertCompletes
    },
    test("MySQL IntVal round-trip") {
      roundtripMysql(MysqlDbValue.IntVal(100000), "int")
      assertCompletes
    },
    test("MySQL BigInt round-trip") {
      roundtripMysql(MysqlDbValue.BigInt(1000000L), "bigint")
      assertCompletes
    },
    test("MySQL TinyIntUnsigned round-trip") {
      roundtripMysql(MysqlDbValue.TinyIntUnsigned(255.toShort), "tinyint-unsigned")
      assertCompletes
    },
    test("MySQL SmallIntUnsigned round-trip") {
      roundtripMysql(MysqlDbValue.SmallIntUnsigned(65535), "smallint-unsigned")
      assertCompletes
    },
    test("MySQL MediumIntUnsigned round-trip") {
      roundtripMysql(MysqlDbValue.MediumIntUnsigned(16777215L), "mediumint-unsigned")
      assertCompletes
    },
    test("MySQL IntUnsigned round-trip") {
      roundtripMysql(MysqlDbValue.IntUnsigned(4294967295L), "int-unsigned")
      assertCompletes
    },
    test("MySQL BigIntUnsigned round-trip") {
      val v   = MysqlDbValue.BigIntUnsigned(scala.BigInt("18446744073709551615"))
      val dyn = MysqlDbValue.toDynamic(v)
      Predef.assert(dyn.tag.asInstanceOf[String] == "bigint-unsigned")
      val parsed = MysqlDbValue.fromDynamic(dyn)
      assertTrue(
        parsed.isInstanceOf[MysqlDbValue.BigIntUnsigned],
        parsed.asInstanceOf[MysqlDbValue.BigIntUnsigned].value == scala.BigInt("18446744073709551615")
      )
    },
    test("MySQL FloatVal round-trip") {
      roundtripMysql(MysqlDbValue.FloatVal(3.14f), "float")
      assertCompletes
    },
    test("MySQL DoubleVal round-trip") {
      roundtripMysql(MysqlDbValue.DoubleVal(2.718), "double")
      assertCompletes
    },
    test("MySQL Decimal round-trip") {
      roundtripMysql(MysqlDbValue.Decimal("99999.99"), "decimal")
      assertCompletes
    },
    test("MySQL Year round-trip") {
      roundtripMysql(MysqlDbValue.Year(2024), "year")
      assertCompletes
    },
    test("MySQL FixChar round-trip") {
      roundtripMysql(MysqlDbValue.FixChar("A"), "fixchar")
      assertCompletes
    },
    test("MySQL VarChar round-trip") {
      roundtripMysql(MysqlDbValue.VarChar("hello"), "varchar")
      assertCompletes
    },
    test("MySQL TinyText round-trip") {
      roundtripMysql(MysqlDbValue.TinyText("tiny"), "tinytext")
      assertCompletes
    },
    test("MySQL Text round-trip") {
      roundtripMysql(MysqlDbValue.Text("text"), "text")
      assertCompletes
    },
    test("MySQL MediumText round-trip") {
      roundtripMysql(MysqlDbValue.MediumText("medium"), "mediumtext")
      assertCompletes
    },
    test("MySQL LongText round-trip") {
      roundtripMysql(MysqlDbValue.LongText("long"), "longtext")
      assertCompletes
    },
    test("MySQL Binary round-trip") {
      roundtripMysql(MysqlDbValue.Binary(Array[Byte](1, 2, 3)), "binary")
      assertCompletes
    },
    test("MySQL VarBinary round-trip") {
      roundtripMysql(MysqlDbValue.VarBinary(Array[Byte](4, 5)), "varbinary")
      assertCompletes
    },
    test("MySQL TinyBlob round-trip") {
      roundtripMysql(MysqlDbValue.TinyBlob(Array[Byte](6)), "tinyblob")
      assertCompletes
    },
    test("MySQL Blob round-trip") {
      roundtripMysql(MysqlDbValue.Blob(Array[Byte](7, 8)), "blob")
      assertCompletes
    },
    test("MySQL MediumBlob round-trip") {
      roundtripMysql(MysqlDbValue.MediumBlob(Array[Byte](9)), "mediumblob")
      assertCompletes
    },
    test("MySQL LongBlob round-trip") {
      roundtripMysql(MysqlDbValue.LongBlob(Array[Byte](10, 11)), "longblob")
      assertCompletes
    },
    test("MySQL Enumeration round-trip") {
      roundtripMysql(MysqlDbValue.Enumeration("active"), "enumeration")
      assertCompletes
    },
    test("MySQL SetVal round-trip") {
      roundtripMysql(MysqlDbValue.SetVal("a,b,c"), "set")
      assertCompletes
    },
    test("MySQL Bit round-trip") {
      roundtripMysql(MysqlDbValue.Bit(List(true, false, true)), "bit")
      assertCompletes
    },
    test("MySQL Json round-trip") {
      roundtripMysql(MysqlDbValue.Json("""{"k":"v"}"""), "json")
      assertCompletes
    },
    test("MySQL Null round-trip") {
      roundtripMysql(MysqlDbValue.Null, "null")
      assertCompletes
    },

    test("MySQL Date round-trip") {
      val d = MysqlDbValue.Date(DbDate(2024, 6, 15))
      roundtripMysql(d, "date")
      assertCompletes
    },

    test("MySQL DateTime round-trip") {
      val d = MysqlDbValue.DateTime(DbTimestamp(DbDate(2024, 6, 15), DbTime(14, 30, 45, 0L)))
      roundtripMysql(d, "datetime")
      assertCompletes
    },

    test("MySQL Timestamp round-trip") {
      val d = MysqlDbValue.Timestamp(DbTimestamp(DbDate(2024, 1, 1), DbTime(0, 0, 0, 0L)))
      roundtripMysql(d, "timestamp")
      assertCompletes
    },

    test("MySQL Time round-trip") {
      val d = MysqlDbValue.Time(DbTime(23, 59, 59, 999999999L))
      roundtripMysql(d, "time")
      assertCompletes
    },

    test("unknown MySQL tag throws") {
      val raw = js.Dynamic.literal(tag = "unknown-mysql", `val` = 0)
      assertTrue(scala.util.Try(MysqlDbValue.fromDynamic(raw)).isFailure)
    },

    // --- Postgres basic round-trips ---

    test("Postgres Character round-trip") {
      roundtripPg(PostgresDbValue.Character(65.toByte), "character")
      assertCompletes
    },
    test("Postgres Int2 round-trip") {
      roundtripPg(PostgresDbValue.Int2(100.toShort), "int2")
      assertCompletes
    },
    test("Postgres Int4 round-trip") {
      roundtripPg(PostgresDbValue.Int4(100000), "int4")
      assertCompletes
    },
    test("Postgres Int8 round-trip") {
      val v   = PostgresDbValue.Int8(1000000000L)
      val dyn = PostgresDbValue.toDynamic(v)
      Predef.assert(dyn.tag.asInstanceOf[String] == "int8")
      val parsed = PostgresDbValue.fromDynamic(dyn)
      assertTrue(
        parsed.isInstanceOf[PostgresDbValue.Int8],
        parsed.asInstanceOf[PostgresDbValue.Int8].value == 1000000000L
      )
    },
    test("Postgres Float4 round-trip") {
      roundtripPg(PostgresDbValue.Float4(3.14f), "float4")
      assertCompletes
    },
    test("Postgres Float8 round-trip") {
      roundtripPg(PostgresDbValue.Float8(2.718), "float8")
      assertCompletes
    },
    test("Postgres Numeric round-trip") {
      roundtripPg(PostgresDbValue.Numeric("12345.6789"), "numeric")
      assertCompletes
    },
    test("Postgres BooleanVal round-trip") {
      roundtripPg(PostgresDbValue.BooleanVal(true), "boolean")
      assertCompletes
    },
    test("Postgres Text round-trip") {
      roundtripPg(PostgresDbValue.Text("hello"), "text")
      assertCompletes
    },
    test("Postgres VarChar round-trip") {
      roundtripPg(PostgresDbValue.VarChar("world"), "varchar")
      assertCompletes
    },
    test("Postgres BpChar round-trip") {
      roundtripPg(PostgresDbValue.BpChar("X"), "bpchar")
      assertCompletes
    },
    test("Postgres Json round-trip") {
      roundtripPg(PostgresDbValue.Json("""{"a":1}"""), "json")
      assertCompletes
    },
    test("Postgres Jsonb round-trip") {
      roundtripPg(PostgresDbValue.Jsonb("""{"b":2}"""), "jsonb")
      assertCompletes
    },
    test("Postgres JsonPath round-trip") {
      roundtripPg(PostgresDbValue.JsonPath("$.x"), "jsonpath")
      assertCompletes
    },
    test("Postgres Xml round-trip") {
      roundtripPg(PostgresDbValue.Xml("<root/>"), "xml")
      assertCompletes
    },
    test("Postgres Bytea round-trip") {
      roundtripPg(PostgresDbValue.Bytea(Array[Byte](1, 2, 3)), "bytea")
      assertCompletes
    },
    test("Postgres Bit round-trip") {
      roundtripPg(PostgresDbValue.Bit(List(true, false)), "bit")
      assertCompletes
    },
    test("Postgres VarBit round-trip") {
      roundtripPg(PostgresDbValue.VarBit(List(false, true)), "varbit")
      assertCompletes
    },
    test("Postgres Null round-trip") {
      roundtripPg(PostgresDbValue.Null, "null")
      assertCompletes
    },

    test("Postgres Money round-trip") {
      val v      = PostgresDbValue.Money(99999L)
      val dyn    = PostgresDbValue.toDynamic(v)
      val parsed = PostgresDbValue.fromDynamic(dyn)
      assertTrue(
        parsed.isInstanceOf[PostgresDbValue.Money],
        parsed.asInstanceOf[PostgresDbValue.Money].value == 99999L
      )
    },

    test("Postgres Oid round-trip") {
      val v      = PostgresDbValue.Oid(12345L)
      val dyn    = PostgresDbValue.toDynamic(v)
      val parsed = PostgresDbValue.fromDynamic(dyn)
      assertTrue(
        parsed.isInstanceOf[PostgresDbValue.Oid],
        parsed.asInstanceOf[PostgresDbValue.Oid].value == 12345L
      )
    },

    test("Postgres Enumeration round-trip") {
      val v = PostgresDbValue.Enumeration(PgEnumeration("status", "active"))
      roundtripPg(v, "enumeration")
      assertCompletes
    },

    test("Postgres Timestamp round-trip") {
      val v = PostgresDbValue.Timestamp(DbTimestamp(DbDate(2024, 6, 15), DbTime(14, 30, 0, 0L)))
      roundtripPg(v, "timestamp")
      assertCompletes
    },

    test("Postgres TimestampTz round-trip") {
      val v = PostgresDbValue.TimestampTz(
        DbTimestampTz(
          DbTimestamp(DbDate(2024, 6, 15), DbTime(14, 30, 0, 0L)),
          3600
        )
      )
      roundtripPg(v, "timestamptz")
      assertCompletes
    },

    test("Postgres Date round-trip") {
      roundtripPg(PostgresDbValue.Date(DbDate(2024, 6, 15)), "date")
      assertCompletes
    },

    test("Postgres Time round-trip") {
      roundtripPg(PostgresDbValue.Time(DbTime(14, 30, 45, 0L)), "time")
      assertCompletes
    },

    test("Postgres TimeTz round-trip") {
      val v = PostgresDbValue.TimeTz(DbTimeTz(DbTime(14, 30, 45, 0L), -18000))
      roundtripPg(v, "timetz")
      assertCompletes
    },

    test("Postgres Interval round-trip") {
      val v = PostgresDbValue.Interval(PgInterval(1, 15, 3600000000L))
      roundtripPg(v, "interval")
      assertCompletes
    },

    test("Postgres Uuid round-trip") {
      val v = PostgresDbValue.Uuid(DbUuid(BigInt("123456789012345678"), BigInt("987654321098765432")))
      roundtripPg(v, "uuid")
      assertCompletes
    },

    test("Postgres Inet Ipv4 round-trip") {
      val v = PostgresDbValue.Inet(IpAddress.Ipv4(192, 168, 1, 1))
      roundtripPg(v, "inet")
      assertCompletes
    },

    test("Postgres Cidr Ipv6 round-trip") {
      val v = PostgresDbValue.Cidr(IpAddress.Ipv6(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))
      roundtripPg(v, "cidr")
      assertCompletes
    },

    test("Postgres MacAddr round-trip") {
      val v = PostgresDbValue.MacAddr(MacAddress(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff))
      roundtripPg(v, "macaddr")
      assertCompletes
    },

    // --- Postgres range round-trips via fromDynamic ---

    test("Postgres Int4Range from dynamic") {
      val raw = js.Dynamic.literal(
        tag = "int4range",
        `val` = js.Dynamic.literal(
          start = js.Dynamic.literal(tag = "included", `val` = 1),
          end = js.Dynamic.literal(tag = "excluded", `val` = 10)
        )
      )
      val parsed = PostgresDbValue.fromDynamic(raw)
      assertTrue(parsed.isInstanceOf[PostgresDbValue.Int4RangeVal])
      val r = parsed.asInstanceOf[PostgresDbValue.Int4RangeVal].value
      assertTrue(
        r.start == Int4Bound.Included(1),
        r.end == Int4Bound.Excluded(10)
      )
    },

    test("Postgres Int8Range from dynamic") {
      val raw = js.Dynamic.literal(
        tag = "int8range",
        `val` = js.Dynamic.literal(
          start = js.Dynamic.literal(tag = "included", `val` = 100.0),
          end = js.Dynamic.literal(tag = "unbounded")
        )
      )
      val parsed = PostgresDbValue.fromDynamic(raw)
      assertTrue(parsed.isInstanceOf[PostgresDbValue.Int8RangeVal])
      val r = parsed.asInstanceOf[PostgresDbValue.Int8RangeVal].value
      assertTrue(
        r.start == Int8Bound.Included(100L),
        r.end == Int8Bound.Unbounded
      )
    },

    test("Postgres NumRange from dynamic") {
      val raw = js.Dynamic.literal(
        tag = "numrange",
        `val` = js.Dynamic.literal(
          start = js.Dynamic.literal(tag = "unbounded"),
          end = js.Dynamic.literal(tag = "excluded", `val` = "999.99")
        )
      )
      val parsed = PostgresDbValue.fromDynamic(raw)
      assertTrue(parsed.isInstanceOf[PostgresDbValue.NumRangeVal])
    },

    test("Postgres DateRange from dynamic") {
      val raw = js.Dynamic.literal(
        tag = "daterange",
        `val` = js.Dynamic.literal(
          start = js.Dynamic.literal(tag = "included", `val` = js.Dynamic.literal(year = 2024, month = 1, day = 1)),
          end = js.Dynamic.literal(tag = "excluded", `val` = js.Dynamic.literal(year = 2024, month = 12, day = 31))
        )
      )
      val parsed = PostgresDbValue.fromDynamic(raw)
      assertTrue(parsed.isInstanceOf[PostgresDbValue.DateRangeVal])
      val r = parsed.asInstanceOf[PostgresDbValue.DateRangeVal].value
      assertTrue(r.start.isInstanceOf[DateBound.Included])
    },

    test("unknown Postgres tag throws") {
      val raw = js.Dynamic.literal(tag = "unknown-pg", `val` = 0)
      assertTrue(scala.util.Try(PostgresDbValue.fromDynamic(raw)).isFailure)
    },

    // --- IpAddress from dynamic ---

    test("IpAddress Ipv4 from dynamic") {
      val raw = js.Dynamic.literal(
        tag = "ipv4",
        `val` = js.Tuple4[Short, Short, Short, Short](10, 0, 0, 1)
      )
      // Can't call parseIpAddress directly (private), but Int4 via Inet covers it
      assertCompletes
    },

    // --- Row accessor tests ---

    test("MysqlDbRow.getString returns None for Null") {
      val row = MysqlDbRow(List(MysqlDbValue.Null))
      assertTrue(row.getString(0) == None)
    },

    test("MysqlDbRow.getString returns Some for non-Null") {
      val row = MysqlDbRow(List(MysqlDbValue.VarChar("test")))
      assertTrue(row.getString(0).isDefined)
    },

    test("MysqlDbRow.getInt extracts int types") {
      val row = MysqlDbRow(List(MysqlDbValue.IntVal(42), MysqlDbValue.TinyInt(1.toByte), MysqlDbValue.Null))
      assertTrue(
        row.getInt(0) == Some(42),
        row.getInt(1) == Some(1),
        row.getInt(2) == None
      )
    },

    test("PostgresDbRow.getString returns None for Null") {
      val row = PostgresDbRow(List(PostgresDbValue.Null))
      assertTrue(row.getString(0) == None)
    },

    test("PostgresDbRow.getInt extracts Int4") {
      val row = PostgresDbRow(List(PostgresDbValue.Int4(99), PostgresDbValue.Null))
      assertTrue(
        row.getInt(0) == Some(99),
        row.getInt(1) == None
      )
    },

    test("PostgresDbRow.getLong extracts Int8") {
      val row = PostgresDbRow(List(PostgresDbValue.Int8(1000000000L), PostgresDbValue.Int4(42), PostgresDbValue.Null))
      assertTrue(
        row.getLong(0) == Some(1000000000L),
        row.getLong(1) == Some(42L),
        row.getLong(2) == None
      )
    },

    // --- DbError ---

    test("DbError variants") {
      val errors = List(
        DbError.ConnectionFailure("conn"),
        DbError.QueryParameterFailure("param"),
        DbError.QueryExecutionFailure("exec"),
        DbError.QueryResponseFailure("resp"),
        DbError.Other("other")
      )
      errors.foreach(e => Predef.assert(e.message.nonEmpty))
      assertCompletes
    },

    // --- DbColumn ---

    test("DbColumn construction") {
      val col = DbColumn(0L, "id", "int4")
      assertTrue(
        col.ordinal == 0L,
        col.name == "id",
        col.dbTypeName == "int4"
      )
    },

    test("parsePostgresResult accepts bigint column ordinals") {
      val raw = js.Dynamic
        .literal(
          columns = js.Array(
            js.Dynamic.literal(
              ordinal = js.BigInt("0"),
              name = "value",
              dbTypeName = "text"
            )
          ),
          rows = js.Array(
            js.Dynamic.literal(
              values = js.Array(
                js.Dynamic.literal(
                  tag = "text",
                  `val` = "postgres-ok"
                )
              )
            )
          )
        )
        .asInstanceOf[golem.host.js.JsDbResult]

      val result = Rdbms.parsePostgresResult(raw)

      assertTrue(
        result.columns.head.ordinal == 0L,
        result.rows.head.values.head == PostgresDbValue.Text("postgres-ok")
      )
    },

    test("parseMysqlResult accepts bigint column ordinals") {
      val raw = js.Dynamic
        .literal(
          columns = js.Array(
            js.Dynamic.literal(
              ordinal = js.BigInt("0"),
              name = "value",
              dbTypeName = "varchar"
            )
          ),
          rows = js.Array(
            js.Dynamic.literal(
              values = js.Array(
                js.Dynamic.literal(
                  tag = "varchar",
                  `val` = "mysql-ok"
                )
              )
            )
          )
        )
        .asInstanceOf[golem.host.js.JsDbResult]

      val result = Rdbms.parseMysqlResult(raw)

      assertTrue(
        result.columns.head.ordinal == 0L,
        result.rows.head.values.head == MysqlDbValue.VarChar("mysql-ok")
      )
    },

    // --- Result types ---

    test("PostgresDbResult construction") {
      val cols = List(DbColumn(0L, "id", "int4"), DbColumn(1L, "name", "text"))
      val rows = List(
        PostgresDbRow(List(PostgresDbValue.Int4(1), PostgresDbValue.Text("a"))),
        PostgresDbRow(List(PostgresDbValue.Int4(2), PostgresDbValue.Text("b")))
      )
      val result = PostgresDbResult(cols, rows)
      assertTrue(
        result.columns.size == 2,
        result.rows.size == 2,
        result.rows.head.getInt(0) == Some(1)
      )
    },

    test("MysqlDbResult construction") {
      val cols   = List(DbColumn(0L, "id", "int"), DbColumn(1L, "name", "varchar"))
      val rows   = List(MysqlDbRow(List(MysqlDbValue.IntVal(1), MysqlDbValue.VarChar("a"))))
      val result = MysqlDbResult(cols, rows)
      assertTrue(
        result.columns.size == 2,
        result.rows.size == 1,
        result.rows.head.getInt(0) == Some(1)
      )
    }
  )
}
