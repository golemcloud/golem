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
  case object BoolType                                                       extends SchemaTypeBody
  final case class S8Type(restrictions: Option[NumericRestrictions] = None)  extends SchemaTypeBody
  final case class S16Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class S32Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class S64Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class U8Type(restrictions: Option[NumericRestrictions] = None)  extends SchemaTypeBody
  final case class U16Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class U32Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class U64Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class F32Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  final case class F64Type(restrictions: Option[NumericRestrictions] = None) extends SchemaTypeBody
  case object CharType                                                       extends SchemaTypeBody
  case object StringType                                                     extends SchemaTypeBody

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

  def ref(id: String): SchemaType                                = st(RefType(id))
  def bool: SchemaType                                           = st(BoolType)
  def s8: SchemaType                                             = s8(None)
  def s8(restrictions: Option[NumericRestrictions]): SchemaType  = st(S8Type(restrictions.flatMap(_.normalize)))
  def s16: SchemaType                                            = s16(None)
  def s16(restrictions: Option[NumericRestrictions]): SchemaType = st(S16Type(restrictions.flatMap(_.normalize)))
  def s32: SchemaType                                            = s32(None)
  def s32(restrictions: Option[NumericRestrictions]): SchemaType = st(S32Type(restrictions.flatMap(_.normalize)))
  def s64: SchemaType                                            = s64(None)
  def s64(restrictions: Option[NumericRestrictions]): SchemaType = st(S64Type(restrictions.flatMap(_.normalize)))
  def u8: SchemaType                                             = u8(None)
  def u8(restrictions: Option[NumericRestrictions]): SchemaType  = st(U8Type(restrictions.flatMap(_.normalize)))
  def u16: SchemaType                                            = u16(None)
  def u16(restrictions: Option[NumericRestrictions]): SchemaType = st(U16Type(restrictions.flatMap(_.normalize)))
  def u32: SchemaType                                            = u32(None)
  def u32(restrictions: Option[NumericRestrictions]): SchemaType = st(U32Type(restrictions.flatMap(_.normalize)))
  def u64: SchemaType                                            = u64(None)
  def u64(restrictions: Option[NumericRestrictions]): SchemaType = st(U64Type(restrictions.flatMap(_.normalize)))
  def f32: SchemaType                                            = f32(None)
  def f32(restrictions: Option[NumericRestrictions]): SchemaType = st(F32Type(restrictions.flatMap(_.normalize)))
  def f64: SchemaType                                            = f64(None)
  def f64(restrictions: Option[NumericRestrictions]): SchemaType = st(F64Type(restrictions.flatMap(_.normalize)))
  def char: SchemaType                                           = st(CharType)
  def string: SchemaType                                         = st(StringType)

  def record(fields: List[NamedFieldType]): SchemaType                       = st(RecordType(fields))
  def variant(cases: List[VariantCaseType]): SchemaType                      = st(VariantType(cases))
  def `enum`(cases: List[String]): SchemaType                                = st(EnumType(cases))
  def flags(names: List[String]): SchemaType                                 = st(FlagsType(names))
  def tuple(elements: List[SchemaType]): SchemaType                          = st(TupleType(elements))
  def list(element: SchemaType): SchemaType                                  = st(ListType(element))
  def fixedList(element: SchemaType, length: Int): SchemaType                = st(FixedListType(element, length))
  def map(key: SchemaType, value: SchemaType): SchemaType                    = st(MapType(key, value))
  def option(element: SchemaType): SchemaType                                = st(OptionType(element))
  def result(ok: Option[SchemaType], err: Option[SchemaType]): SchemaType    = st(ResultType(ok, err))
  def datetime: SchemaType                                                   = st(DatetimeType)
  def duration: SchemaType                                                   = st(DurationType)
  def secret(inner: SchemaType, category: Option[String] = None): SchemaType =
    st(SecretType(SecretSpec(inner, category)))

  def field(name: String, body: SchemaType): NamedFieldType                          = NamedFieldType(name, body)
  def variantCase(name: String, payload: Option[SchemaType] = None): VariantCaseType =
    VariantCaseType(name, payload)
}
