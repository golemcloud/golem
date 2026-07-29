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

package golem.tool

import golem.schema._

/**
 * Refinement overlays applied by the tool authoring layer onto a parameter's
 * derived schema type (`regex`/`minLength`/`maxLength`, path/url specs, numeric
 * bounds). Each overlay only touches the authored fields; a schema kind that
 * has no such restrictions is rejected with
 * [[ToolBuildError.RefinementTypeMismatch]] rather than silently rewritten.
 */
object ToolRefinement {
  import SchemaTypeBody._

  /**
   * Applies text refinements to a text-backed schema. A plain `string` is
   * promoted to `text` (it has nowhere to store restrictions); a `text`
   * overlays only the authored fields.
   */
  def refineText(
    base: SchemaType,
    regex: Option[String],
    minLen: Option[Int],
    maxLen: Option[Int]
  ): Either[ToolBuildError, SchemaType] = {
    val base0 = base.body match {
      case TextType(restrictions) => Right(restrictions)
      case StringType             => Right(TextRestrictions.empty)
      case other                  =>
        Left(ToolBuildError.RefinementTypeMismatch("text", schemaKindName(other)))
    }
    base0.map { restrictions =>
      val refined = restrictions.copy(
        regex = regex.orElse(restrictions.regex),
        minLength = minLen.orElse(restrictions.minLength),
        maxLength = maxLen.orElse(restrictions.maxLength)
      )
      SchemaType(TextType(refined), base.metadata)
    }
  }

  /**
   * Applies path refinements to a `path` schema type. `acceptsStdio` is not a
   * property of the schema type; it lives on the positional that carries the
   * path. A non-`path` schema is rejected (no `string`→`path` coercion).
   */
  def refinePath(
    base: SchemaType,
    direction: Option[PathDirection],
    kind: Option[PathKind],
    mime: Option[List[String]]
  ): Either[ToolBuildError, SchemaType] =
    base.body match {
      case PathType(spec) =>
        val refined = spec.copy(
          direction = direction.getOrElse(spec.direction),
          kind = kind.getOrElse(spec.kind),
          allowedMimeTypes = mime.map(Some(_)).getOrElse(spec.allowedMimeTypes)
        )
        Right(SchemaType(PathType(refined), base.metadata))
      case other =>
        Left(ToolBuildError.RefinementTypeMismatch("path", schemaKindName(other)))
    }

  /**
   * Applies url refinements to a `url` schema type. A non-`url` schema is
   * rejected (no `string`→`url` coercion).
   */
  def refineUrl(
    base: SchemaType,
    schemes: Option[List[String]]
  ): Either[ToolBuildError, SchemaType] =
    base.body match {
      case UrlType(restrictions) =>
        val refined = restrictions.copy(
          allowedSchemes = schemes.map(Some(_)).getOrElse(restrictions.allowedSchemes)
        )
        Right(SchemaType(UrlType(refined), base.metadata))
      case other =>
        Left(ToolBuildError.RefinementTypeMismatch("url", schemaKindName(other)))
    }

  /**
   * Applies numeric refinements (`min`/`max`/`unit`) to one of the ten numeric
   * primitive schema variants, preserving the exact variant and overlaying only
   * the authored fields onto any existing restrictions.
   */
  def refineNumeric(
    base: SchemaType,
    min: Option[NumericBound],
    max: Option[NumericBound],
    unit: Option[String]
  ): Either[ToolBuildError, SchemaType] = {
    def overlay(existing: Option[NumericRestrictions]): Option[NumericRestrictions] = {
      val current = existing.getOrElse(NumericRestrictions.empty)
      current
        .copy(
          min = min.orElse(current.min),
          max = max.orElse(current.max),
          unit = unit.orElse(current.unit)
        )
        .normalize
    }
    val refined = base.body match {
      case S8Type(r)  => Right(S8Type(overlay(r)): SchemaTypeBody)
      case S16Type(r) => Right(S16Type(overlay(r)): SchemaTypeBody)
      case S32Type(r) => Right(S32Type(overlay(r)): SchemaTypeBody)
      case S64Type(r) => Right(S64Type(overlay(r)): SchemaTypeBody)
      case U8Type(r)  => Right(U8Type(overlay(r)): SchemaTypeBody)
      case U16Type(r) => Right(U16Type(overlay(r)): SchemaTypeBody)
      case U32Type(r) => Right(U32Type(overlay(r)): SchemaTypeBody)
      case U64Type(r) => Right(U64Type(overlay(r)): SchemaTypeBody)
      case F32Type(r) => Right(F32Type(overlay(r)): SchemaTypeBody)
      case F64Type(r) => Right(F64Type(overlay(r)): SchemaTypeBody)
      case other      =>
        Left(ToolBuildError.RefinementTypeMismatch("numeric", schemaKindName(other)))
    }
    refined.map(body => SchemaType(body, base.metadata))
  }

  /**
   * Converts a float bound literal into canonical [[NumericBound.FloatBits]],
   * rejecting non-finite values (`NaN`/`inf`) so they surface as a
   * [[ToolBuildError]] from the descriptor build. `-0.0` is normalized to
   * `+0.0` so canonical bits are stable for equality.
   */
  def floatBound(value: Double): Either[ToolBuildError, NumericBound] =
    if (java.lang.Double.isNaN(value) || java.lang.Double.isInfinite(value))
      Left(ToolBuildError.InvalidNumericBound("float bound must be finite"))
    else
      Right(NumericBound.FloatBits(java.lang.Double.doubleToLongBits(value + 0.0d)))

  /**
   * A short, stable name for a schema kind, used in
   * [[ToolBuildError.RefinementTypeMismatch]] messages.
   */
  def schemaKindName(body: SchemaTypeBody): String =
    body match {
      case _: RefType => "ref"
      case BoolType   => "bool"
      case _: S8Type | _: S16Type | _: S32Type | _: S64Type | _: U8Type | _: U16Type | _: U32Type | _: U64Type |
          _: F32Type | _: F64Type =>
        "numeric"
      case CharType          => "char"
      case StringType        => "string"
      case _: TextType       => "text"
      case _: PathType       => "path"
      case _: UrlType        => "url"
      case _: RecordType     => "record"
      case _: VariantType    => "variant"
      case _: EnumType       => "enum"
      case _: FlagsType      => "flags"
      case _: TupleType      => "tuple"
      case _: ListType       => "list"
      case _: FixedListType  => "fixed-list"
      case _: MapType        => "map"
      case _: OptionType     => "option"
      case _: ResultType     => "result"
      case _: BinaryType     => "binary"
      case DatetimeType      => "datetime"
      case DurationType      => "duration"
      case _: QuantityType   => "quantity"
      case _: UnionType      => "union"
      case _: SecretType     => "secret"
      case _: QuotaTokenType => "quota-token"
      case _: FutureType     => "future"
      case _: StreamType     => "stream"
    }
}
