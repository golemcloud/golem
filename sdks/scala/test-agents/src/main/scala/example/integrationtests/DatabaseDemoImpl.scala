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

package example.integrationtests

import golem.host.Rdbms
import golem.host.Rdbms._
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class DatabaseDemoImpl(@unused private val name: String) extends DatabaseDemo {

  override def postgresDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Postgres Demo ===\n")

    Rdbms.Postgres.open("host=localhost dbname=demo") match {
      case Left(err) =>
        err match {
          case DbError.ConnectionFailure(msg)     => sb.append(s"Connection failed: $msg\n")
          case DbError.QueryParameterFailure(msg) => sb.append(s"Param error: $msg\n")
          case DbError.QueryExecutionFailure(msg) => sb.append(s"Exec error: $msg\n")
          case DbError.QueryResponseFailure(msg)  => sb.append(s"Response error: $msg\n")
          case DbError.Other(msg)                 => sb.append(s"Other error: $msg\n")
        }

      case Right(conn) =>
        val params: List[PostgresDbValue] = List(
          PostgresDbValue.Text("hello"),
          PostgresDbValue.Int4(42),
          PostgresDbValue.BooleanVal(true)
        )

        conn.query("SELECT $1::text, $2::int, $3::bool", params) match {
          case Left(err)     => sb.append(s"Query error: ${err.message}\n")
          case Right(result) =>
            sb.append(s"Columns: ${result.columns.map(c => s"${c.name}:${c.dbTypeName}").mkString(", ")}\n")
            result.rows.foreach { row =>
              sb.append(s"  getString(0)=${row.getString(0)}\n")
              sb.append(s"  getInt(1)=${row.getInt(1)}\n")
              row.values.foreach {
                case PostgresDbValue.Text(s)       => sb.append(s"    text: $s\n")
                case PostgresDbValue.Int4(n)       => sb.append(s"    int4: $n\n")
                case PostgresDbValue.Int8(n)       => sb.append(s"    int8: $n\n")
                case PostgresDbValue.BooleanVal(b) => sb.append(s"    bool: $b\n")
                case PostgresDbValue.Timestamp(ts) =>
                  sb.append(s"    ts: ${ts.date.year}-${ts.date.month}-${ts.date.day}\n")
                case PostgresDbValue.Uuid(u) => sb.append(s"    uuid: hi=${u.highBits} lo=${u.lowBits}\n")
                case PostgresDbValue.Json(j) => sb.append(s"    json: $j\n")
                case PostgresDbValue.Null    => sb.append("    null\n")
                case other                   => sb.append(s"    other: $other\n")
              }
            }
        }

        conn.beginTransaction() match {
          case Left(err) => sb.append(s"Transaction error: ${err.message}\n")
          case Right(tx) =>
            tx.execute("INSERT INTO demo (name) VALUES ($1)", List(PostgresDbValue.Text("test"))) match {
              case Left(err)    => sb.append(s"Execute error: ${err.message}\n")
              case Right(count) => sb.append(s"Inserted $count rows\n")
            }
            tx.rollback() match {
              case Left(err) => sb.append(s"Rollback error: ${err.message}\n")
              case Right(_)  => sb.append("Rolled back.\n")
            }
        }
    }

    sb.toString()
  }

  override def mysqlDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== MySQL Demo ===\n")

    Rdbms.Mysql.open("host=localhost;database=demo") match {
      case Left(err)   => sb.append(s"Connection error: ${err.message}\n")
      case Right(conn) =>
        val params: List[MysqlDbValue] = List(
          MysqlDbValue.VarChar("param1"),
          MysqlDbValue.IntVal(42),
          MysqlDbValue.Decimal("99.99")
        )

        conn.query("SELECT ?, ?, ?", params) match {
          case Left(err)     => sb.append(s"Query error: ${err.message}\n")
          case Right(result) =>
            sb.append(s"Columns: ${result.columns.map(_.name).mkString(", ")}\n")
            result.rows.foreach { row =>
              sb.append(s"  getString(0)=${row.getString(0)}\n")
              sb.append(s"  getInt(1)=${row.getInt(1)}\n")
              row.values.foreach {
                case MysqlDbValue.Text(s)      => sb.append(s"    text: $s\n")
                case MysqlDbValue.VarChar(s)   => sb.append(s"    varchar: $s\n")
                case MysqlDbValue.IntVal(n)    => sb.append(s"    int: $n\n")
                case MysqlDbValue.BigInt(n)    => sb.append(s"    bigint: $n\n")
                case MysqlDbValue.DateTime(ts) => sb.append(s"    datetime: ${ts.date.year}\n")
                case MysqlDbValue.Json(j)      => sb.append(s"    json: $j\n")
                case MysqlDbValue.Null         => sb.append("    null\n")
                case other                     => sb.append(s"    other: $other\n")
              }
            }
        }

        conn.beginTransaction() match {
          case Left(err) => sb.append(s"Transaction error: ${err.message}\n")
          case Right(tx) =>
            tx.execute("INSERT INTO demo (name) VALUES (?)", List(MysqlDbValue.VarChar("test"))) match {
              case Left(err)    => sb.append(s"Execute error: ${err.message}\n")
              case Right(count) => sb.append(s"Inserted $count rows\n")
            }
            tx.commit() match {
              case Left(err) => sb.append(s"Commit error: ${err.message}\n")
              case Right(_)  => sb.append("Committed.\n")
            }
        }
    }

    sb.toString()
  }

  override def typeShowcase(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Type Showcase ===\n")

    val date   = DbDate(2024, 6, 15)
    val time   = DbTime(14, 30, 45, 123456789L)
    val ts     = DbTimestamp(date, time)
    val tstz   = DbTimestampTz(ts, 3600)
    val timeTz = DbTimeTz(time, -18000)
    val uuid   = DbUuid(BigInt("123456789012345678"), BigInt("987654321098765432"))

    sb.append(s"DbDate: $date\n")
    sb.append(s"DbTime: $time\n")
    sb.append(s"DbTimestamp: $ts\n")
    sb.append(s"DbTimestampTz: $tstz\n")
    sb.append(s"DbTimeTz: $timeTz\n")
    sb.append(s"DbUuid: $uuid\n")

    val ipv4 = IpAddress.Ipv4(192, 168, 1, 1)
    val ipv6 = IpAddress.Ipv6(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)
    val mac  = MacAddress(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff)
    sb.append(s"IPv4: $ipv4\n")
    sb.append(s"IPv6: $ipv6\n")
    sb.append(s"MAC: $mac\n")

    val interval = PgInterval(1, 15, 3600000000L)
    val pgEnum   = PgEnumeration("status", "active")
    sb.append(s"PgInterval: $interval\n")
    sb.append(s"PgEnumeration: $pgEnum\n")

    val range = Int4Range(Int4Bound.Included(1), Int4Bound.Excluded(10))
    sb.append(s"Int4Range: $range\n")

    val i8range = Int8Range(Int8Bound.Included(100L), Int8Bound.Unbounded)
    sb.append(s"Int8Range: $i8range\n")

    val numRange = NumRange(NumBound.Unbounded, NumBound.Excluded("999.99"))
    sb.append(s"NumRange: $numRange\n")

    val tsRange = TsRange(TsBound.Included(ts), TsBound.Excluded(ts))
    sb.append(s"TsRange: $tsRange\n")

    val tstzRange = TsTzRange(TsTzBound.Included(tstz), TsTzBound.Unbounded)
    sb.append(s"TsTzRange: $tstzRange\n")

    val dateRange = DateRange(DateBound.Included(date), DateBound.Excluded(date))
    sb.append(s"DateRange: $dateRange\n")

    val composite = PgComposite("point", List(PostgresDbValue.Int4(1), PostgresDbValue.Int4(2)))
    val domain    = PgDomain("email", PostgresDbValue.Text("a@b.com"))
    sb.append(s"PgComposite: $composite\n")
    sb.append(s"PgDomain: $domain\n")

    val pgRange = PgRange(
      "custom_range",
      PgValuesRange(
        PgValueBound.Included(PostgresDbValue.Int4(1)),
        PgValueBound.Excluded(PostgresDbValue.Int4(100))
      )
    )
    sb.append(s"PgRange: $pgRange\n")

    sb.toString()
  }
}
