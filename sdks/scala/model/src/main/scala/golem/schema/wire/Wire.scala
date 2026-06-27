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

package golem.schema.wire

import golem.schema._

// Flat-with-indices carrier ADT mirroring `golem:core/types@2.0.0`
// (`schema-graph` / `schema-value-tree` / `typed-schema-value`) exactly.
//
// This is host-agnostic plain Scala (cross JVM+JS) so the recursive <-> flat
// conversion is unit-testable on the JVM. The JS layer (core module) maps this
// ADT to/from the `@JSImport` facades produced by wasm-rquickjs; that mapping is
// purely mechanical field renaming.
//
// Indices are `Int` (`type-node-index` / `value-node-index` / `def-index` are
// all `s32` in the WIT).

// ============================================================
// Schema type / graph
// ============================================================

final case class WitSchemaGraph(
  typeNodes: Vector[WitSchemaTypeNode],
  defs: Vector[WitSchemaTypeDef],
  root: Int
)

final case class WitSchemaTypeDef(id: String, name: Option[String], body: Int)

final case class WitSchemaTypeNode(body: WitSchemaTypeBody, metadata: MetadataEnvelope)

final case class WitNamedFieldType(name: String, body: Int, metadata: MetadataEnvelope)
final case class WitVariantCaseType(name: String, payload: Option[Int], metadata: MetadataEnvelope)
final case class WitFixedListSpec(element: Int, length: Int)
final case class WitMapSpec(key: Int, value: Int)
final case class WitResultSpec(ok: Option[Int], err: Option[Int])
final case class WitUnionSpec(branches: Vector[WitUnionBranch])
final case class WitUnionBranch(
  tag: String,
  body: Int,
  discriminator: DiscriminatorRule,
  metadata: MetadataEnvelope
)

sealed trait WitSchemaTypeBody extends Product with Serializable
object WitSchemaTypeBody {
  final case class RefType(defIndex: Int) extends WitSchemaTypeBody

  case object BoolType                                                       extends WitSchemaTypeBody
  final case class S8Type(restrictions: Option[NumericRestrictions] = None)  extends WitSchemaTypeBody
  final case class S16Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class S32Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class S64Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class U8Type(restrictions: Option[NumericRestrictions] = None)  extends WitSchemaTypeBody
  final case class U16Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class U32Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class U64Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class F32Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  final case class F64Type(restrictions: Option[NumericRestrictions] = None) extends WitSchemaTypeBody
  case object CharType                                                       extends WitSchemaTypeBody
  case object StringType                                                     extends WitSchemaTypeBody

  final case class RecordType(fields: Vector[WitNamedFieldType])  extends WitSchemaTypeBody
  final case class VariantType(cases: Vector[WitVariantCaseType]) extends WitSchemaTypeBody
  final case class EnumType(cases: Vector[String])                extends WitSchemaTypeBody
  final case class FlagsType(names: Vector[String])               extends WitSchemaTypeBody
  final case class TupleType(elements: Vector[Int])               extends WitSchemaTypeBody
  final case class ListType(element: Int)                         extends WitSchemaTypeBody
  final case class FixedListType(spec: WitFixedListSpec)          extends WitSchemaTypeBody
  final case class MapType(spec: WitMapSpec)                      extends WitSchemaTypeBody
  final case class OptionType(element: Int)                       extends WitSchemaTypeBody
  final case class ResultType(spec: WitResultSpec)                extends WitSchemaTypeBody

  final case class TextType(restrictions: TextRestrictions)     extends WitSchemaTypeBody
  final case class BinaryType(restrictions: BinaryRestrictions) extends WitSchemaTypeBody
  final case class PathType(spec: PathSpec)                     extends WitSchemaTypeBody
  final case class UrlType(restrictions: UrlRestrictions)       extends WitSchemaTypeBody
  case object DatetimeType                                      extends WitSchemaTypeBody
  case object DurationType                                      extends WitSchemaTypeBody
  final case class QuantityType(spec: QuantitySpec)             extends WitSchemaTypeBody

  final case class UnionType(spec: WitUnionSpec) extends WitSchemaTypeBody

  final case class SecretType(spec: SecretSpec)         extends WitSchemaTypeBody
  final case class QuotaTokenType(spec: QuotaTokenSpec) extends WitSchemaTypeBody

  final case class FutureType(element: Option[Int]) extends WitSchemaTypeBody
  final case class StreamType(element: Option[Int]) extends WitSchemaTypeBody
}

// ============================================================
// Schema value
// ============================================================

final case class WitSchemaValueTree(valueNodes: Vector[WitSchemaValueNode], root: Int)

final case class WitVariantValuePayload(caseIndex: Int, payload: Option[Int])
final case class WitMapEntry(key: Int, value: Int)
final case class WitTextValuePayload(text: String, language: Option[String])
final case class WitBinaryValuePayload(bytes: Vector[Byte], mimeType: Option[String])
final case class WitDurationValuePayload(nanoseconds: Long)
final case class WitUnionValuePayload(tag: String, body: Int)
final case class WitSecretValuePayload(secretRef: String)

sealed trait WitResultValuePayload extends Product with Serializable
object WitResultValuePayload {
  final case class OkValue(value: Option[Int])  extends WitResultValuePayload
  final case class ErrValue(value: Option[Int]) extends WitResultValuePayload
}

sealed trait WitSchemaValueNode extends Product with Serializable
object WitSchemaValueNode {
  final case class BoolValue(value: Boolean)  extends WitSchemaValueNode
  final case class S8Value(value: Byte)       extends WitSchemaValueNode
  final case class S16Value(value: Short)     extends WitSchemaValueNode
  final case class S32Value(value: Int)       extends WitSchemaValueNode
  final case class S64Value(value: Long)      extends WitSchemaValueNode
  final case class U8Value(value: Int)        extends WitSchemaValueNode
  final case class U16Value(value: Int)       extends WitSchemaValueNode
  final case class U32Value(value: Long)      extends WitSchemaValueNode
  final case class U64Value(value: Long)      extends WitSchemaValueNode
  final case class F32Value(value: Float)     extends WitSchemaValueNode
  final case class F64Value(value: Double)    extends WitSchemaValueNode
  final case class CharValue(value: Int)      extends WitSchemaValueNode
  final case class StringValue(value: String) extends WitSchemaValueNode

  final case class RecordValue(fields: Vector[Int])              extends WitSchemaValueNode
  final case class VariantValue(payload: WitVariantValuePayload) extends WitSchemaValueNode
  final case class EnumValue(caseIndex: Int)                     extends WitSchemaValueNode
  final case class FlagsValue(flags: Vector[Boolean])            extends WitSchemaValueNode
  final case class TupleValue(elements: Vector[Int])             extends WitSchemaValueNode
  final case class ListValue(elements: Vector[Int])              extends WitSchemaValueNode
  final case class FixedListValue(elements: Vector[Int])         extends WitSchemaValueNode
  final case class MapValue(entries: Vector[WitMapEntry])        extends WitSchemaValueNode
  final case class OptionValue(value: Option[Int])               extends WitSchemaValueNode
  final case class ResultValue(payload: WitResultValuePayload)   extends WitSchemaValueNode

  final case class TextValue(payload: WitTextValuePayload)         extends WitSchemaValueNode
  final case class BinaryValue(payload: WitBinaryValuePayload)     extends WitSchemaValueNode
  final case class PathValue(value: String)                        extends WitSchemaValueNode
  final case class UrlValue(value: String)                         extends WitSchemaValueNode
  final case class DatetimeValue(value: Datetime)                  extends WitSchemaValueNode
  final case class DurationValue(payload: WitDurationValuePayload) extends WitSchemaValueNode
  final case class QuantityValueNode(value: QuantityValue)         extends WitSchemaValueNode

  final case class UnionValue(payload: WitUnionValuePayload) extends WitSchemaValueNode

  final case class SecretValue(payload: WitSecretValuePayload) extends WitSchemaValueNode

  /**
   * Flat carrier for an owned `quota-token` resource (`schema-value-node ::
   * quota-token-handle(own<quota-token>)`). Carries the opaque affine handle
   * unchanged; the take-once transfer happens at the JS host boundary.
   */
  final case class QuotaTokenHandle(handle: GuestQuotaTokenHandle) extends WitSchemaValueNode
}

final case class WitTypedSchemaValue(graph: WitSchemaGraph, value: WitSchemaValueTree)
