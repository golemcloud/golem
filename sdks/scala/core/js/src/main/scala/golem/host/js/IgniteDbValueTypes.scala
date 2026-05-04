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
import scala.scalajs.js.typedarray.Uint8Array

// ---------------------------------------------------------------------------
// golem:rdbms/ignite2@1.5.0  –  JS facade traits for Ignite DbValue
// ---------------------------------------------------------------------------

@js.native
sealed trait JsIgniteDbValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsIgniteDbValueWithValue extends JsIgniteDbValue {
  @JSName("val") def value: js.Any = js.native
}

object JsIgniteDbValue {
  def dbNull: JsIgniteDbValue                                              = JsShape.tagOnly[JsIgniteDbValue]("db-null")
  def dbBoolean(v: Boolean): JsIgniteDbValue                               = JsShape.tagged[JsIgniteDbValue]("db-boolean", v.asInstanceOf[js.Any])
  def dbByte(v: Int): JsIgniteDbValue                                      = JsShape.tagged[JsIgniteDbValue]("db-byte", v.asInstanceOf[js.Any])
  def dbShort(v: Int): JsIgniteDbValue                                     = JsShape.tagged[JsIgniteDbValue]("db-short", v.asInstanceOf[js.Any])
  def dbInt(v: Int): JsIgniteDbValue                                       = JsShape.tagged[JsIgniteDbValue]("db-int", v.asInstanceOf[js.Any])
  def dbLong(v: js.BigInt): JsIgniteDbValue                                = JsShape.tagged[JsIgniteDbValue]("db-long", v.asInstanceOf[js.Any])
  def dbFloat(v: Double): JsIgniteDbValue                                  = JsShape.tagged[JsIgniteDbValue]("db-float", v.asInstanceOf[js.Any])
  def dbDouble(v: Double): JsIgniteDbValue                                 = JsShape.tagged[JsIgniteDbValue]("db-double", v.asInstanceOf[js.Any])
  def dbChar(v: Int): JsIgniteDbValue                                      = JsShape.tagged[JsIgniteDbValue]("db-char", v.asInstanceOf[js.Any])
  def dbString(v: String): JsIgniteDbValue                                 = JsShape.tagged[JsIgniteDbValue]("db-string", v.asInstanceOf[js.Any])
  def dbUuid(v: js.Tuple2[js.BigInt, js.BigInt]): JsIgniteDbValue          = JsShape.tagged[JsIgniteDbValue]("db-uuid", v.asInstanceOf[js.Any])
  def dbDate(v: js.BigInt): JsIgniteDbValue                                = JsShape.tagged[JsIgniteDbValue]("db-date", v.asInstanceOf[js.Any])
  def dbTimestamp(v: js.Tuple2[js.BigInt, Int]): JsIgniteDbValue           = JsShape.tagged[JsIgniteDbValue]("db-timestamp", v.asInstanceOf[js.Any])
  def dbTime(v: js.BigInt): JsIgniteDbValue                                = JsShape.tagged[JsIgniteDbValue]("db-time", v.asInstanceOf[js.Any])
  def dbDecimal(v: String): JsIgniteDbValue                                = JsShape.tagged[JsIgniteDbValue]("db-decimal", v.asInstanceOf[js.Any])
  def dbByteArray(v: Uint8Array): JsIgniteDbValue                          = JsShape.tagged[JsIgniteDbValue]("db-byte-array", v.asInstanceOf[js.Any])
}
