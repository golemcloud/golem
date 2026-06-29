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

package golem.bridge.runtime

/**
 * Recursive, in-memory mirror of the server's schema-native `SchemaValue`. The
 * value tree is structurally driven by the schema: record payload order matches
 * the schema's field order, variant/enum carry a case index, union carries the
 * discriminator's literal tag. The value side does not redundantly carry field
 * names, case names, or named-ref identifiers — those come from the schema.
 *
 * Mirrors the Golem Scala SDK's `golem.schema.SchemaValue` so the generated
 * client's type mapping stays close to the SDK. Capability nodes (quantity,
 * secret, quota-token) and the streaming nodes are not part of the agent IO
 * surface exercised by the bridge and are intentionally omitted.
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
  final case class DatetimeValue(value: String)                               extends SchemaValue
  final case class DurationValue(nanoseconds: Long)                           extends SchemaValue

  // Discriminated union
  final case class UnionValue(unionTag: String, body: SchemaValue) extends SchemaValue
}

/** An entry of a [[SchemaValue.MapValue]]. */
final case class SchemaMapEntry(key: SchemaValue, value: SchemaValue)

/** Result payload: exactly one of ok/err, each optionally carrying a value. */
sealed trait SchemaResult extends Product with Serializable
object SchemaResult {
  final case class Ok(value: Option[SchemaValue])  extends SchemaResult
  final case class Err(value: Option[SchemaValue]) extends SchemaResult
}
