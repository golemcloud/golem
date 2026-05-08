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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:rdbms/types@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

@js.native
sealed trait JsDbDate extends js.Object {
  def year: Int  = js.native
  def month: Int = js.native
  def day: Int   = js.native
}

object JsDbDate {
  def apply(year: Int, month: Int, day: Int): JsDbDate =
    js.Dynamic.literal("year" -> year, "month" -> month, "day" -> day).asInstanceOf[JsDbDate]
}

@js.native
sealed trait JsDbTime extends js.Object {
  def hour: Int       = js.native
  def minute: Int     = js.native
  def second: Int     = js.native
  def nanosecond: Int = js.native
}

object JsDbTime {
  def apply(hour: Int, minute: Int, second: Int, nanosecond: Int): JsDbTime =
    js.Dynamic
      .literal("hour" -> hour, "minute" -> minute, "second" -> second, "nanosecond" -> nanosecond)
      .asInstanceOf[JsDbTime]
}

@js.native
sealed trait JsDbTimestamp extends js.Object {
  def date: JsDbDate = js.native
  def time: JsDbTime = js.native
}

object JsDbTimestamp {
  def apply(date: JsDbDate, time: JsDbTime): JsDbTimestamp =
    js.Dynamic.literal("date" -> date, "time" -> time).asInstanceOf[JsDbTimestamp]
}

@js.native
sealed trait JsDbTimestampTz extends js.Object {
  def timestamp: JsDbTimestamp = js.native
  def offset: Int              = js.native
}

object JsDbTimestampTz {
  def apply(timestamp: JsDbTimestamp, offset: Int): JsDbTimestampTz =
    js.Dynamic.literal("timestamp" -> timestamp, "offset" -> offset).asInstanceOf[JsDbTimestampTz]
}

@js.native
sealed trait JsDbTimeTz extends js.Object {
  def time: JsDbTime = js.native
  def offset: Int    = js.native
}

object JsDbTimeTz {
  def apply(time: JsDbTime, offset: Int): JsDbTimeTz =
    js.Dynamic.literal("time" -> time, "offset" -> offset).asInstanceOf[JsDbTimeTz]
}

@js.native
sealed trait JsDbUuid extends js.Object {
  def highBits: js.BigInt = js.native
  def lowBits: js.BigInt  = js.native
}

object JsDbUuid {
  def apply(highBits: js.BigInt, lowBits: js.BigInt): JsDbUuid =
    js.Dynamic.literal("highBits" -> highBits, "lowBits" -> lowBits).asInstanceOf[JsDbUuid]
}

// --- IpAddress  –  tagged union ---

@js.native
sealed trait JsIpAddress extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsIpAddressIpv4 extends JsIpAddress {
  @JSName("val") def value: js.Tuple4[Int, Int, Int, Int] = js.native
}

@js.native
sealed trait JsIpAddressIpv6 extends JsIpAddress {
  @JSName("val") def value: js.Array[Int] = js.native
}

object JsIpAddress {
  def ipv4(a: Int, b: Int, c: Int, d: Int): JsIpAddress =
    JsShape.tagged[JsIpAddress]("ipv4", js.Tuple4(a, b, c, d))

  def ipv6(segments: js.Array[Int]): JsIpAddress =
    JsShape.tagged[JsIpAddress]("ipv6", segments)
}

// --- MacAddress ---

@js.native
sealed trait JsMacAddress extends js.Object {
  def octets: js.Array[Int] = js.native
}

object JsMacAddress {
  def apply(octets: js.Array[Int]): JsMacAddress =
    js.Dynamic.literal("octets" -> octets).asInstanceOf[JsMacAddress]
}

// --- DbConnection / DbTransaction resources ---

@js.native
private[golem] sealed trait JsDbConnection extends js.Object {
  def query(statement: String, params: js.Array[js.Any]): js.Any   = js.native
  def execute(statement: String, params: js.Array[js.Any]): js.Any = js.native
  def beginTransaction(): JsDbTransaction                          = js.native
}

@js.native
private[golem] sealed trait JsDbTransaction extends js.Object {
  def query(statement: String, params: js.Array[js.Any]): js.Any   = js.native
  def execute(statement: String, params: js.Array[js.Any]): js.Any = js.native
  def commit(): Unit                                               = js.native
  def rollback(): Unit                                             = js.native
}

// --- DbResult / DbColumn / DbRow ---

@js.native
private[golem] sealed trait JsDbColumn extends js.Object {
  def ordinal: js.Any    = js.native
  def name: String       = js.native
  def dbTypeName: String = js.native
}

@js.native
private[golem] sealed trait JsDbRow extends js.Object {
  def values: js.Array[js.Any] = js.native
}

@js.native
private[golem] sealed trait JsDbResult extends js.Object {
  def columns: js.Array[JsDbColumn] = js.native
  def rows: js.Array[JsDbRow]       = js.native
}
