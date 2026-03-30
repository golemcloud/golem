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
// golem:rdbms/mysql@1.5.0  –  JS facade traits for MySQL DbValue
// ---------------------------------------------------------------------------

@js.native
sealed trait JsMysqlDbValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsMysqlDbValueWithValue extends JsMysqlDbValue {
  @JSName("val") def value: js.Any = js.native
}

object JsMysqlDbValue {
  def boolean(v: Boolean): JsMysqlDbValue     = JsShape.tagged[JsMysqlDbValue]("boolean", v.asInstanceOf[js.Any])
  def tinyint(v: Int): JsMysqlDbValue         = JsShape.tagged[JsMysqlDbValue]("tinyint", v.asInstanceOf[js.Any])
  def smallint(v: Int): JsMysqlDbValue        = JsShape.tagged[JsMysqlDbValue]("smallint", v.asInstanceOf[js.Any])
  def mediumint(v: Int): JsMysqlDbValue       = JsShape.tagged[JsMysqlDbValue]("mediumint", v.asInstanceOf[js.Any])
  def int(v: Int): JsMysqlDbValue             = JsShape.tagged[JsMysqlDbValue]("int", v.asInstanceOf[js.Any])
  def bigint(v: js.BigInt): JsMysqlDbValue    = JsShape.tagged[JsMysqlDbValue]("bigint", v.asInstanceOf[js.Any])
  def tinyintUnsigned(v: Int): JsMysqlDbValue =
    JsShape.tagged[JsMysqlDbValue]("tinyint-unsigned", v.asInstanceOf[js.Any])
  def smallintUnsigned(v: Int): JsMysqlDbValue =
    JsShape.tagged[JsMysqlDbValue]("smallint-unsigned", v.asInstanceOf[js.Any])
  def mediumintUnsigned(v: Int): JsMysqlDbValue =
    JsShape.tagged[JsMysqlDbValue]("mediumint-unsigned", v.asInstanceOf[js.Any])
  def intUnsigned(v: Double): JsMysqlDbValue       = JsShape.tagged[JsMysqlDbValue]("int-unsigned", v.asInstanceOf[js.Any])
  def bigintUnsigned(v: js.BigInt): JsMysqlDbValue =
    JsShape.tagged[JsMysqlDbValue]("bigint-unsigned", v.asInstanceOf[js.Any])
  def float(v: Double): JsMysqlDbValue            = JsShape.tagged[JsMysqlDbValue]("float", v.asInstanceOf[js.Any])
  def double(v: Double): JsMysqlDbValue           = JsShape.tagged[JsMysqlDbValue]("double", v.asInstanceOf[js.Any])
  def decimal(v: String): JsMysqlDbValue          = JsShape.tagged[JsMysqlDbValue]("decimal", v.asInstanceOf[js.Any])
  def date(v: JsDbDate): JsMysqlDbValue           = JsShape.tagged[JsMysqlDbValue]("date", v.asInstanceOf[js.Any])
  def datetime(v: JsDbTimestamp): JsMysqlDbValue  = JsShape.tagged[JsMysqlDbValue]("datetime", v.asInstanceOf[js.Any])
  def timestamp(v: JsDbTimestamp): JsMysqlDbValue = JsShape.tagged[JsMysqlDbValue]("timestamp", v.asInstanceOf[js.Any])
  def time(v: JsDbTime): JsMysqlDbValue           = JsShape.tagged[JsMysqlDbValue]("time", v.asInstanceOf[js.Any])
  def year(v: Int): JsMysqlDbValue                = JsShape.tagged[JsMysqlDbValue]("year", v.asInstanceOf[js.Any])
  def fixchar(v: String): JsMysqlDbValue          = JsShape.tagged[JsMysqlDbValue]("fixchar", v.asInstanceOf[js.Any])
  def varchar(v: String): JsMysqlDbValue          = JsShape.tagged[JsMysqlDbValue]("varchar", v.asInstanceOf[js.Any])
  def tinytext(v: String): JsMysqlDbValue         = JsShape.tagged[JsMysqlDbValue]("tinytext", v.asInstanceOf[js.Any])
  def text(v: String): JsMysqlDbValue             = JsShape.tagged[JsMysqlDbValue]("text", v.asInstanceOf[js.Any])
  def mediumtext(v: String): JsMysqlDbValue       = JsShape.tagged[JsMysqlDbValue]("mediumtext", v.asInstanceOf[js.Any])
  def longtext(v: String): JsMysqlDbValue         = JsShape.tagged[JsMysqlDbValue]("longtext", v.asInstanceOf[js.Any])
  def binary(v: Uint8Array): JsMysqlDbValue       = JsShape.tagged[JsMysqlDbValue]("binary", v.asInstanceOf[js.Any])
  def varbinary(v: Uint8Array): JsMysqlDbValue    = JsShape.tagged[JsMysqlDbValue]("varbinary", v.asInstanceOf[js.Any])
  def tinyblob(v: Uint8Array): JsMysqlDbValue     = JsShape.tagged[JsMysqlDbValue]("tinyblob", v.asInstanceOf[js.Any])
  def blob(v: Uint8Array): JsMysqlDbValue         = JsShape.tagged[JsMysqlDbValue]("blob", v.asInstanceOf[js.Any])
  def mediumblob(v: Uint8Array): JsMysqlDbValue   = JsShape.tagged[JsMysqlDbValue]("mediumblob", v.asInstanceOf[js.Any])
  def longblob(v: Uint8Array): JsMysqlDbValue     = JsShape.tagged[JsMysqlDbValue]("longblob", v.asInstanceOf[js.Any])
  def enumeration(v: String): JsMysqlDbValue      = JsShape.tagged[JsMysqlDbValue]("enumeration", v.asInstanceOf[js.Any])
  def set(v: String): JsMysqlDbValue              = JsShape.tagged[JsMysqlDbValue]("set", v.asInstanceOf[js.Any])
  def bit(v: js.Array[Boolean]): JsMysqlDbValue   = JsShape.tagged[JsMysqlDbValue]("bit", v.asInstanceOf[js.Any])
  def json(v: String): JsMysqlDbValue             = JsShape.tagged[JsMysqlDbValue]("json", v.asInstanceOf[js.Any])
  def `null`: JsMysqlDbValue                      = JsShape.tagOnly[JsMysqlDbValue]("null")
}
