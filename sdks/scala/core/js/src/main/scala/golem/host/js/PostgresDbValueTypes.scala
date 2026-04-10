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
// golem:rdbms/postgres@1.5.0  –  JS facade traits for PostgreSQL DbValue
// ---------------------------------------------------------------------------

// --- Bound tagged unions ---

@js.native
sealed trait JsInt4Bound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsInt4BoundWithValue extends JsInt4Bound { @JSName("val") def value: Int = js.native }

object JsInt4Bound {
  def included(v: Int): JsInt4Bound = JsShape.tagged[JsInt4Bound]("included", v.asInstanceOf[js.Any])
  def excluded(v: Int): JsInt4Bound = JsShape.tagged[JsInt4Bound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsInt4Bound        = JsShape.tagOnly[JsInt4Bound]("unbounded")
}

@js.native
sealed trait JsInt8Bound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsInt8BoundWithValue extends JsInt8Bound { @JSName("val") def value: js.BigInt = js.native }

object JsInt8Bound {
  def included(v: js.BigInt): JsInt8Bound = JsShape.tagged[JsInt8Bound]("included", v.asInstanceOf[js.Any])
  def excluded(v: js.BigInt): JsInt8Bound = JsShape.tagged[JsInt8Bound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsInt8Bound              = JsShape.tagOnly[JsInt8Bound]("unbounded")
}

@js.native
sealed trait JsNumBound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsNumBoundWithValue extends JsNumBound { @JSName("val") def value: String = js.native }

object JsNumBound {
  def included(v: String): JsNumBound = JsShape.tagged[JsNumBound]("included", v.asInstanceOf[js.Any])
  def excluded(v: String): JsNumBound = JsShape.tagged[JsNumBound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsNumBound           = JsShape.tagOnly[JsNumBound]("unbounded")
}

@js.native
sealed trait JsTsBound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsTsBoundWithValue extends JsTsBound { @JSName("val") def value: JsDbTimestamp = js.native }

object JsTsBound {
  def included(v: JsDbTimestamp): JsTsBound = JsShape.tagged[JsTsBound]("included", v.asInstanceOf[js.Any])
  def excluded(v: JsDbTimestamp): JsTsBound = JsShape.tagged[JsTsBound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsTsBound                  = JsShape.tagOnly[JsTsBound]("unbounded")
}

@js.native
sealed trait JsTsTzBound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsTsTzBoundWithValue extends JsTsTzBound { @JSName("val") def value: JsDbTimestampTz = js.native }

object JsTsTzBound {
  def included(v: JsDbTimestampTz): JsTsTzBound = JsShape.tagged[JsTsTzBound]("included", v.asInstanceOf[js.Any])
  def excluded(v: JsDbTimestampTz): JsTsTzBound = JsShape.tagged[JsTsTzBound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsTsTzBound                    = JsShape.tagOnly[JsTsTzBound]("unbounded")
}

@js.native
sealed trait JsDateBound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsDateBoundWithValue extends JsDateBound { @JSName("val") def value: JsDbDate = js.native }

object JsDateBound {
  def included(v: JsDbDate): JsDateBound = JsShape.tagged[JsDateBound]("included", v.asInstanceOf[js.Any])
  def excluded(v: JsDbDate): JsDateBound = JsShape.tagged[JsDateBound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsDateBound             = JsShape.tagOnly[JsDateBound]("unbounded")
}

// --- Range records ---

@js.native
sealed trait JsInt4Range extends js.Object {
  def start: JsInt4Bound = js.native
  def end: JsInt4Bound   = js.native
}

object JsInt4Range {
  def apply(start: JsInt4Bound, end: JsInt4Bound): JsInt4Range =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsInt4Range]
}

@js.native
sealed trait JsInt8Range extends js.Object {
  def start: JsInt8Bound = js.native
  def end: JsInt8Bound   = js.native
}

object JsInt8Range {
  def apply(start: JsInt8Bound, end: JsInt8Bound): JsInt8Range =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsInt8Range]
}

@js.native
sealed trait JsNumRange extends js.Object {
  def start: JsNumBound = js.native
  def end: JsNumBound   = js.native
}

object JsNumRange {
  def apply(start: JsNumBound, end: JsNumBound): JsNumRange =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsNumRange]
}

@js.native
sealed trait JsTsRange extends js.Object {
  def start: JsTsBound = js.native
  def end: JsTsBound   = js.native
}

object JsTsRange {
  def apply(start: JsTsBound, end: JsTsBound): JsTsRange =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsTsRange]
}

@js.native
sealed trait JsTsTzRange extends js.Object {
  def start: JsTsTzBound = js.native
  def end: JsTsTzBound   = js.native
}

object JsTsTzRange {
  def apply(start: JsTsTzBound, end: JsTsTzBound): JsTsTzRange =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsTsTzRange]
}

@js.native
sealed trait JsDateRange extends js.Object {
  def start: JsDateBound = js.native
  def end: JsDateBound   = js.native
}

object JsDateRange {
  def apply(start: JsDateBound, end: JsDateBound): JsDateRange =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsDateRange]
}

// --- Other supporting types ---

@js.native
sealed trait JsPgInterval extends js.Object {
  def months: Int             = js.native
  def days: Int               = js.native
  def microseconds: js.BigInt = js.native
}

object JsPgInterval {
  def apply(months: Int, days: Int, microseconds: js.BigInt): JsPgInterval =
    js.Dynamic.literal("months" -> months, "days" -> days, "microseconds" -> microseconds).asInstanceOf[JsPgInterval]
}

@js.native
sealed trait JsPgEnumerationType extends js.Object {
  def name: String = js.native
}

object JsPgEnumerationType {
  def apply(name: String): JsPgEnumerationType =
    js.Dynamic.literal("name" -> name).asInstanceOf[JsPgEnumerationType]
}

@js.native
sealed trait JsPgEnumeration extends js.Object {
  def name: String  = js.native
  def value: String = js.native
}

object JsPgEnumeration {
  def apply(name: String, value: String): JsPgEnumeration =
    js.Dynamic.literal("name" -> name, "value" -> value).asInstanceOf[JsPgEnumeration]
}

@js.native
sealed trait JsPgSparseVec extends js.Object {
  def dim: Int                 = js.native
  def indices: js.Array[Int]   = js.native
  def values: js.Array[Double] = js.native
}

object JsPgSparseVec {
  def apply(dim: Int, indices: js.Array[Int], values: js.Array[Double]): JsPgSparseVec =
    js.Dynamic.literal("dim" -> dim, "indices" -> indices, "values" -> values).asInstanceOf[JsPgSparseVec]
}

@js.native
sealed trait JsLazyDbValue extends js.Object {
  def get(): js.Any = js.native
}

@js.native
sealed trait JsPgComposite extends js.Object {
  def name: String                    = js.native
  def values: js.Array[JsLazyDbValue] = js.native
}

object JsPgComposite {
  def apply(name: String, values: js.Array[JsLazyDbValue]): JsPgComposite =
    js.Dynamic.literal("name" -> name, "values" -> values).asInstanceOf[JsPgComposite]
}

@js.native
sealed trait JsPgDomain extends js.Object {
  def name: String         = js.native
  def value: JsLazyDbValue = js.native
}

object JsPgDomain {
  def apply(name: String, value: JsLazyDbValue): JsPgDomain =
    js.Dynamic.literal("name" -> name, "value" -> value).asInstanceOf[JsPgDomain]
}

@js.native
sealed trait JsValueBound extends js.Object { def tag: String = js.native }
@js.native
sealed trait JsValueBoundWithValue extends JsValueBound { @JSName("val") def value: JsLazyDbValue = js.native }

object JsValueBound {
  def included(v: JsLazyDbValue): JsValueBound = JsShape.tagged[JsValueBound]("included", v.asInstanceOf[js.Any])
  def excluded(v: JsLazyDbValue): JsValueBound = JsShape.tagged[JsValueBound]("excluded", v.asInstanceOf[js.Any])
  def unbounded: JsValueBound                  = JsShape.tagOnly[JsValueBound]("unbounded")
}

@js.native
sealed trait JsValuesRange extends js.Object {
  def start: JsValueBound = js.native
  def end: JsValueBound   = js.native
}

object JsValuesRange {
  def apply(start: JsValueBound, end: JsValueBound): JsValuesRange =
    js.Dynamic.literal("start" -> start, "end" -> end).asInstanceOf[JsValuesRange]
}

@js.native
sealed trait JsPgRange extends js.Object {
  def name: String         = js.native
  def value: JsValuesRange = js.native
}

object JsPgRange {
  def apply(name: String, value: JsValuesRange): JsPgRange =
    js.Dynamic.literal("name" -> name, "value" -> value).asInstanceOf[JsPgRange]
}

// --- Main PostgreSQL DbValue tagged union ---

@js.native
sealed trait JsPostgresDbValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPostgresDbValueWithValue extends JsPostgresDbValue {
  @JSName("val") def value: js.Any = js.native
}

object JsPostgresDbValue {
  def character(v: Int): JsPostgresDbValue           = JsShape.tagged[JsPostgresDbValue]("character", v.asInstanceOf[js.Any])
  def int2(v: Int): JsPostgresDbValue                = JsShape.tagged[JsPostgresDbValue]("int2", v.asInstanceOf[js.Any])
  def int4(v: Int): JsPostgresDbValue                = JsShape.tagged[JsPostgresDbValue]("int4", v.asInstanceOf[js.Any])
  def int8(v: js.BigInt): JsPostgresDbValue          = JsShape.tagged[JsPostgresDbValue]("int8", v.asInstanceOf[js.Any])
  def float4(v: Double): JsPostgresDbValue           = JsShape.tagged[JsPostgresDbValue]("float4", v.asInstanceOf[js.Any])
  def float8(v: Double): JsPostgresDbValue           = JsShape.tagged[JsPostgresDbValue]("float8", v.asInstanceOf[js.Any])
  def numeric(v: String): JsPostgresDbValue          = JsShape.tagged[JsPostgresDbValue]("numeric", v.asInstanceOf[js.Any])
  def boolean(v: Boolean): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("boolean", v.asInstanceOf[js.Any])
  def text(v: String): JsPostgresDbValue             = JsShape.tagged[JsPostgresDbValue]("text", v.asInstanceOf[js.Any])
  def varchar(v: String): JsPostgresDbValue          = JsShape.tagged[JsPostgresDbValue]("varchar", v.asInstanceOf[js.Any])
  def bpchar(v: String): JsPostgresDbValue           = JsShape.tagged[JsPostgresDbValue]("bpchar", v.asInstanceOf[js.Any])
  def timestamp(v: JsDbTimestamp): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("timestamp", v.asInstanceOf[js.Any])
  def timestamptz(v: JsDbTimestampTz): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("timestamptz", v.asInstanceOf[js.Any])
  def date(v: JsDbDate): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("date", v.asInstanceOf[js.Any])
  def time(v: JsDbTime): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("time", v.asInstanceOf[js.Any])
  def timetz(v: JsDbTimeTz): JsPostgresDbValue     = JsShape.tagged[JsPostgresDbValue]("timetz", v.asInstanceOf[js.Any])
  def interval(v: JsPgInterval): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("interval", v.asInstanceOf[js.Any])
  def bytea(v: Uint8Array): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("bytea", v.asInstanceOf[js.Any])
  def json(v: String): JsPostgresDbValue              = JsShape.tagged[JsPostgresDbValue]("json", v.asInstanceOf[js.Any])
  def jsonb(v: String): JsPostgresDbValue             = JsShape.tagged[JsPostgresDbValue]("jsonb", v.asInstanceOf[js.Any])
  def jsonpath(v: String): JsPostgresDbValue          = JsShape.tagged[JsPostgresDbValue]("jsonpath", v.asInstanceOf[js.Any])
  def xml(v: String): JsPostgresDbValue               = JsShape.tagged[JsPostgresDbValue]("xml", v.asInstanceOf[js.Any])
  def uuid(v: JsDbUuid): JsPostgresDbValue            = JsShape.tagged[JsPostgresDbValue]("uuid", v.asInstanceOf[js.Any])
  def inet(v: JsIpAddress): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("inet", v.asInstanceOf[js.Any])
  def cidr(v: JsIpAddress): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("cidr", v.asInstanceOf[js.Any])
  def macaddr(v: JsMacAddress): JsPostgresDbValue     = JsShape.tagged[JsPostgresDbValue]("macaddr", v.asInstanceOf[js.Any])
  def bit(v: js.Array[Boolean]): JsPostgresDbValue    = JsShape.tagged[JsPostgresDbValue]("bit", v.asInstanceOf[js.Any])
  def varbit(v: js.Array[Boolean]): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("varbit", v.asInstanceOf[js.Any])
  def int4range(v: JsInt4Range): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("int4range", v.asInstanceOf[js.Any])
  def int8range(v: JsInt8Range): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("int8range", v.asInstanceOf[js.Any])
  def numrange(v: JsNumRange): JsPostgresDbValue   = JsShape.tagged[JsPostgresDbValue]("numrange", v.asInstanceOf[js.Any])
  def tsrange(v: JsTsRange): JsPostgresDbValue     = JsShape.tagged[JsPostgresDbValue]("tsrange", v.asInstanceOf[js.Any])
  def tstzrange(v: JsTsTzRange): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("tstzrange", v.asInstanceOf[js.Any])
  def daterange(v: JsDateRange): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("daterange", v.asInstanceOf[js.Any])
  def money(v: js.BigInt): JsPostgresDbValue             = JsShape.tagged[JsPostgresDbValue]("money", v.asInstanceOf[js.Any])
  def oid(v: Int): JsPostgresDbValue                     = JsShape.tagged[JsPostgresDbValue]("oid", v.asInstanceOf[js.Any])
  def enumeration(v: JsPgEnumeration): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("enumeration", v.asInstanceOf[js.Any])
  def composite(v: JsPgComposite): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("composite", v.asInstanceOf[js.Any])
  def domain(v: JsPgDomain): JsPostgresDbValue             = JsShape.tagged[JsPostgresDbValue]("domain", v.asInstanceOf[js.Any])
  def array(v: js.Array[JsLazyDbValue]): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("array", v.asInstanceOf[js.Any])
  def range(v: JsPgRange): JsPostgresDbValue         = JsShape.tagged[JsPostgresDbValue]("range", v.asInstanceOf[js.Any])
  def `null`: JsPostgresDbValue                      = JsShape.tagOnly[JsPostgresDbValue]("null")
  def vector(v: js.Array[Double]): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("vector", v.asInstanceOf[js.Any])
  def halfvec(v: js.Array[Double]): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("halfvec", v.asInstanceOf[js.Any])
  def sparsevec(v: JsPgSparseVec): JsPostgresDbValue =
    JsShape.tagged[JsPostgresDbValue]("sparsevec", v.asInstanceOf[js.Any])
}
