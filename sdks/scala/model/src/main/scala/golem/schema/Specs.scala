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

// Non-recursive leaf structures of the schema model. These mirror the
// corresponding `golem:core/types@2.0.0` records/variants exactly so the WIT
// codecs can pass them through unchanged.
//
// Integer width mapping: WIT `u32`/`s32` map to Scala `Int`, WIT `u64`/`s64`
// map to Scala `Long`. `u64` values store the raw bits (unsigned semantics are
// applied at the rendering layer). Realistic string/byte lengths never exceed
// `Int` range.

/**
 * WIT-shaped datetime (`golem:core/types@2.0.0` `datetime`). Distinct from the
 * SDK's user-facing `golem.Datetime` (epoch millis scheduling type) because the
 * schema model needs the exact wire shape with nanosecond precision.
 */
final case class Datetime(seconds: Long, nanoseconds: Int)

final case class TextRestrictions(
  languages: Option[List[String]] = None,
  minLength: Option[Int] = None,
  maxLength: Option[Int] = None,
  regex: Option[String] = None
)

object TextRestrictions {
  val empty: TextRestrictions = TextRestrictions()
}

final case class BinaryRestrictions(
  mimeTypes: Option[List[String]] = None,
  minBytes: Option[Int] = None,
  maxBytes: Option[Int] = None
)

object BinaryRestrictions {
  val empty: BinaryRestrictions = BinaryRestrictions()
}

sealed trait PathDirection extends Product with Serializable
object PathDirection {
  case object Input  extends PathDirection
  case object Output extends PathDirection
  case object InOut  extends PathDirection
}

sealed trait PathKind extends Product with Serializable
object PathKind {
  case object File      extends PathKind
  case object Directory extends PathKind
  case object Any       extends PathKind
}

final case class PathSpec(
  direction: PathDirection,
  kind: PathKind,
  allowedMimeTypes: Option[List[String]] = None,
  allowedExtensions: Option[List[String]] = None
)

final case class UrlRestrictions(
  allowedSchemes: Option[List[String]] = None,
  allowedHosts: Option[List[String]] = None
)

object UrlRestrictions {
  val empty: UrlRestrictions = UrlRestrictions()
}

/**
 * Fixed-point decimal value with unit: numeric value =
 * `mantissa * 10^(-scale)`.
 */
final case class QuantityValue(mantissa: Long, scale: Int, unit: String)

final case class QuantitySpec(
  baseUnit: String,
  allowedSuffixes: List[String] = Nil,
  min: Option[QuantityValue] = None,
  max: Option[QuantityValue] = None
)

final case class SecretSpec(inner: SchemaType, category: Option[String] = None)

final case class QuotaTokenSpec(resourceName: Option[String] = None)

/** Optional required literal value for a record-shaped union discriminator. */
final case class FieldDiscriminator(fieldName: String, literal: Option[String] = None)

/** How the decoder identifies that a value belongs to a given union branch. */
sealed trait DiscriminatorRule extends Product with Serializable
object DiscriminatorRule {
  final case class Prefix(value: String)                  extends DiscriminatorRule
  final case class Suffix(value: String)                  extends DiscriminatorRule
  final case class Contains(value: String)                extends DiscriminatorRule
  final case class Regex(value: String)                   extends DiscriminatorRule
  final case class FieldEquals(field: FieldDiscriminator) extends DiscriminatorRule
  final case class FieldAbsent(fieldName: String)         extends DiscriminatorRule
}
