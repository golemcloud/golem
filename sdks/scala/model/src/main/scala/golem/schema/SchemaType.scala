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
 * Recursive, in-memory mirror of a `golem:core/types@2.0.0` `schema-type-node`:
 * a structural [[SchemaTypeBody]] plus its [[MetadataEnvelope]].
 *
 * The WIT carrier (`schema-graph`) is flat-with-indices because WIT has no
 * recursive types. This recursive form is what the rest of the SDK works with;
 * [[golem.schema.wire]] converts to and from the flat carrier mechanically.
 */
final case class SchemaType(
  body: SchemaTypeBody,
  metadata: MetadataEnvelope = MetadataEnvelope.empty
)

/** A named field of a [[SchemaTypeBody.RecordType]]. */
final case class NamedFieldType(
  name: String,
  body: SchemaType,
  metadata: MetadataEnvelope = MetadataEnvelope.empty
)

/** A case of a [[SchemaTypeBody.VariantType]]. */
final case class VariantCaseType(
  name: String,
  payload: Option[SchemaType] = None,
  metadata: MetadataEnvelope = MetadataEnvelope.empty
)

/** A branch of a [[SchemaTypeBody.UnionType]] (closed, inferred-tag sum). */
final case class UnionBranch(
  tag: String,
  body: SchemaType,
  discriminator: DiscriminatorRule,
  metadata: MetadataEnvelope = MetadataEnvelope.empty
)

/** The structural body of a [[SchemaType]]. */
sealed trait SchemaTypeBody extends Product with Serializable

object SchemaTypeBody {
  // Reference to a named definition in the enclosing graph.
  final case class RefType(id: String) extends SchemaTypeBody

  // Primitives
  case object BoolType   extends SchemaTypeBody
  case object S8Type     extends SchemaTypeBody
  case object S16Type    extends SchemaTypeBody
  case object S32Type    extends SchemaTypeBody
  case object S64Type    extends SchemaTypeBody
  case object U8Type     extends SchemaTypeBody
  case object U16Type    extends SchemaTypeBody
  case object U32Type    extends SchemaTypeBody
  case object U64Type    extends SchemaTypeBody
  case object F32Type    extends SchemaTypeBody
  case object F64Type    extends SchemaTypeBody
  case object CharType   extends SchemaTypeBody
  case object StringType extends SchemaTypeBody

  // Structural composites
  final case class RecordType(fields: List[NamedFieldType])                    extends SchemaTypeBody
  final case class VariantType(cases: List[VariantCaseType])                   extends SchemaTypeBody
  final case class EnumType(cases: List[String])                               extends SchemaTypeBody
  final case class FlagsType(names: List[String])                              extends SchemaTypeBody
  final case class TupleType(elements: List[SchemaType])                       extends SchemaTypeBody
  final case class ListType(element: SchemaType)                               extends SchemaTypeBody
  final case class FixedListType(element: SchemaType, length: Int)             extends SchemaTypeBody
  final case class MapType(key: SchemaType, value: SchemaType)                 extends SchemaTypeBody
  final case class OptionType(element: SchemaType)                             extends SchemaTypeBody
  final case class ResultType(ok: Option[SchemaType], err: Option[SchemaType]) extends SchemaTypeBody

  // Rich semantic types
  final case class TextType(restrictions: TextRestrictions)     extends SchemaTypeBody
  final case class BinaryType(restrictions: BinaryRestrictions) extends SchemaTypeBody
  final case class PathType(spec: PathSpec)                     extends SchemaTypeBody
  final case class UrlType(restrictions: UrlRestrictions)       extends SchemaTypeBody
  case object DatetimeType                                      extends SchemaTypeBody
  case object DurationType                                      extends SchemaTypeBody
  final case class QuantityType(spec: QuantitySpec)             extends SchemaTypeBody

  // Discriminated union (closed, inferred-tag)
  final case class UnionType(branches: List[UnionBranch]) extends SchemaTypeBody

  // Capability nodes
  final case class SecretType(spec: SecretSpec)         extends SchemaTypeBody
  final case class QuotaTokenType(spec: QuotaTokenSpec) extends SchemaTypeBody

  // WASI P3 stubs (parseable only; no semantics yet)
  final case class FutureType(element: Option[SchemaType]) extends SchemaTypeBody
  final case class StreamType(element: Option[SchemaType]) extends SchemaTypeBody
}

/**
 * Compact constructors for [[SchemaType]]s (anonymous unless registered into a
 * graph via [[SchemaBuilder]]). Mirrors the TS SDK's `t` helpers.
 */
object t {
  import SchemaTypeBody._

  private def st(body: SchemaTypeBody): SchemaType = SchemaType(body)

  def ref(id: String): SchemaType = st(RefType(id))
  def bool: SchemaType            = st(BoolType)
  def s8: SchemaType              = st(S8Type)
  def s16: SchemaType             = st(S16Type)
  def s32: SchemaType             = st(S32Type)
  def s64: SchemaType             = st(S64Type)
  def u8: SchemaType              = st(U8Type)
  def u16: SchemaType             = st(U16Type)
  def u32: SchemaType             = st(U32Type)
  def u64: SchemaType             = st(U64Type)
  def f32: SchemaType             = st(F32Type)
  def f64: SchemaType             = st(F64Type)
  def char: SchemaType            = st(CharType)
  def string: SchemaType          = st(StringType)

  def record(fields: List[NamedFieldType]): SchemaType                    = st(RecordType(fields))
  def variant(cases: List[VariantCaseType]): SchemaType                   = st(VariantType(cases))
  def `enum`(cases: List[String]): SchemaType                             = st(EnumType(cases))
  def flags(names: List[String]): SchemaType                              = st(FlagsType(names))
  def tuple(elements: List[SchemaType]): SchemaType                       = st(TupleType(elements))
  def list(element: SchemaType): SchemaType                               = st(ListType(element))
  def fixedList(element: SchemaType, length: Int): SchemaType             = st(FixedListType(element, length))
  def map(key: SchemaType, value: SchemaType): SchemaType                 = st(MapType(key, value))
  def option(element: SchemaType): SchemaType                             = st(OptionType(element))
  def result(ok: Option[SchemaType], err: Option[SchemaType]): SchemaType = st(ResultType(ok, err))
  def datetime: SchemaType                                                = st(DatetimeType)
  def duration: SchemaType                                                = st(DurationType)

  def field(name: String, body: SchemaType): NamedFieldType                          = NamedFieldType(name, body)
  def variantCase(name: String, payload: Option[SchemaType] = None): VariantCaseType =
    VariantCaseType(name, payload)
}
