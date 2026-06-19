/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.schema

/**
 * Recursive, in-memory mirror of a `golem:core/types@2.0.0`
 * `schema-value-node`.
 *
 * The value tree is structurally driven by the schema: record payload order
 * matches the schema's field order, variant carries a case index, enum carries
 * a case index, union carries the discriminator's literal tag. The value side
 * does not redundantly carry field names, case names, or named-ref identifiers
 * — those come from the schema.
 *
 * `char` values are stored as a Unicode code point (`Int`); `binary` bytes are
 * stored as an immutable `Vector[Byte]` so values have structural equality.
 */
sealed trait SchemaValue extends Product with Serializable

object SchemaValue {
  // Primitives
  final case class BoolValue(value: Boolean)  extends SchemaValue
  final case class S8Value(value: Byte)       extends SchemaValue
  final case class S16Value(value: Short)     extends SchemaValue
  final case class S32Value(value: Int)       extends SchemaValue
  final case class S64Value(value: Long)      extends SchemaValue
  final case class U8Value(value: Int)        extends SchemaValue
  final case class U16Value(value: Int)       extends SchemaValue
  final case class U32Value(value: Long)      extends SchemaValue
  final case class U64Value(value: Long)      extends SchemaValue
  final case class F32Value(value: Float)     extends SchemaValue
  final case class F64Value(value: Double)    extends SchemaValue
  final case class CharValue(value: Int)      extends SchemaValue
  final case class StringValue(value: String) extends SchemaValue

  // Structural composites
  final case class RecordValue(fields: List[SchemaValue])                     extends SchemaValue
  final case class VariantValue(caseIndex: Int, payload: Option[SchemaValue]) extends SchemaValue
  final case class EnumValue(caseIndex: Int)                                  extends SchemaValue
  final case class FlagsValue(flags: List[Boolean])                           extends SchemaValue
  final case class TupleValue(elements: List[SchemaValue])                    extends SchemaValue
  final case class ListValue(elements: List[SchemaValue])                     extends SchemaValue
  final case class FixedListValue(elements: List[SchemaValue])                extends SchemaValue
  final case class MapValue(entries: List[SchemaMapEntry])                    extends SchemaValue
  final case class OptionValue(value: Option[SchemaValue])                    extends SchemaValue
  final case class ResultValue(result: SchemaResult)                          extends SchemaValue

  // Rich semantic
  final case class TextValue(text: String, language: Option[String])          extends SchemaValue
  final case class BinaryValue(bytes: Vector[Byte], mimeType: Option[String]) extends SchemaValue
  final case class PathValue(value: String)                                   extends SchemaValue
  final case class UrlValue(value: String)                                    extends SchemaValue
  final case class DatetimeValue(value: Datetime)                             extends SchemaValue
  final case class DurationValue(nanoseconds: Long)                           extends SchemaValue
  final case class QuantityValueNode(value: QuantityValue)                    extends SchemaValue

  // Discriminated union
  final case class UnionValue(unionTag: String, body: SchemaValue) extends SchemaValue

  // Capability nodes
  final case class SecretValue(secretRef: String)                 extends SchemaValue
  final case class QuotaTokenValue(value: QuotaTokenValuePayload) extends SchemaValue
}

/** An entry of a [[SchemaValue.MapValue]]. */
final case class SchemaMapEntry(key: SchemaValue, value: SchemaValue)

/** Result payload: exactly one of ok/err, each optionally carrying a value. */
sealed trait SchemaResult extends Product with Serializable
object SchemaResult {
  final case class Ok(value: Option[SchemaValue])  extends SchemaResult
  final case class Err(value: Option[SchemaValue]) extends SchemaResult
}

/**
 * Compact constructors for [[SchemaValue]]s. Mirrors the TS SDK's `v` helpers.
 */
object v {
  import SchemaValue._

  def bool(value: Boolean): SchemaValue  = BoolValue(value)
  def s8(value: Byte): SchemaValue       = S8Value(value)
  def s16(value: Short): SchemaValue     = S16Value(value)
  def s32(value: Int): SchemaValue       = S32Value(value)
  def s64(value: Long): SchemaValue      = S64Value(value)
  def u8(value: Int): SchemaValue        = U8Value(value)
  def u16(value: Int): SchemaValue       = U16Value(value)
  def u32(value: Long): SchemaValue      = U32Value(value)
  def u64(value: Long): SchemaValue      = U64Value(value)
  def f32(value: Float): SchemaValue     = F32Value(value)
  def f64(value: Double): SchemaValue    = F64Value(value)
  def char(value: Int): SchemaValue      = CharValue(value)
  def string(value: String): SchemaValue = StringValue(value)

  def record(fields: List[SchemaValue]): SchemaValue                            = RecordValue(fields)
  def variant(caseIndex: Int, payload: Option[SchemaValue] = None): SchemaValue =
    VariantValue(caseIndex, payload)
  def `enum`(caseIndex: Int): SchemaValue                 = EnumValue(caseIndex)
  def flags(flags: List[Boolean]): SchemaValue            = FlagsValue(flags)
  def tuple(elements: List[SchemaValue]): SchemaValue     = TupleValue(elements)
  def list(elements: List[SchemaValue]): SchemaValue      = ListValue(elements)
  def fixedList(elements: List[SchemaValue]): SchemaValue = FixedListValue(elements)
  def map(entries: List[SchemaMapEntry]): SchemaValue     = MapValue(entries)
  def option(value: Option[SchemaValue]): SchemaValue     = OptionValue(value)
  def ok(value: Option[SchemaValue]): SchemaValue         = ResultValue(SchemaResult.Ok(value))
  def err(value: Option[SchemaValue]): SchemaValue        = ResultValue(SchemaResult.Err(value))
}
