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

import zio.blocks.schema.{Modifier, Schema}

import java.net.{URI, URISyntaxException}
import java.time.{Duration => JDuration, Instant}
import scala.annotation.publicInBinary
import scala.collection.immutable.ListMap

final case class GolemPath(value: String) extends AnyVal

object GolemPath {
  val defaultSpec: PathSpec = PathSpec(PathDirection.InOut, PathKind.Any)

  implicit val schema: Schema[GolemPath] = Schema.derived

  implicit val intoSchema: IntoSchema[GolemPath] =
    new IntoSchema[GolemPath] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.PathType(defaultSpec)))
      override def toValue(value: GolemPath): SchemaValue =
        SchemaValue.PathValue(UnicodeText.encodeText("path", value.value))
    }

  implicit val fromSchema: FromSchema[GolemPath] =
    new FromSchema[GolemPath] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, GolemPath] =
        value match {
          case SchemaValue.PathValue(v) => Right(GolemPath(v))
          case other                    => Left(FromSchemaError(s"expected path value for GolemPath, got $other"))
        }
    }
}

private[golem] object UrlText {
  val defaultRestrictions: UrlRestrictions = UrlRestrictions.empty

  def encode(value: URI): String = UnicodeText.encodeText("url", value.toString)

  def parse(value: String): Either[String, URI] =
    try Right(new URI(value))
    catch { case e: URISyntaxException => Left(s"invalid url '$value': ${e.getReason}") }
}

private[golem] object DurationNanos {

  /**
   * The signed nanoseconds of a duration. Throws [[SchemaEncodeError]] outside
   * the s64 range. Computed here because the Scala.js `java.time` overflows at
   * the most negative value.
   */
  def of(value: JDuration): Long = {
    val seconds = value.getSeconds
    try
      if (seconds < 0)
        Math.addExact(Math.multiplyExact(seconds + 1, 1000000000L), value.getNano.toLong - 1000000000L)
      else Math.addExact(Math.multiplyExact(seconds, 1000000000L), value.getNano.toLong)
    catch {
      case _: ArithmeticException => throw SchemaEncodeError(s"Duration is out of i64 nanoseconds range: $value")
    }
  }
}

/**
 * Bare text with an optional language code. Its schema is `text` without
 * restrictions; a callee may publish a refined `text` schema (languages,
 * length, regex), which the server checks.
 */
final case class Text(text: String, language: Option[String] = None)

object Text {
  val defaultRestrictions: TextRestrictions = TextRestrictions.empty

  implicit val schema: Schema[Text] = Schema.derived

  private[golem] def toSchemaValue(value: Text): SchemaValue =
    SchemaValue.TextValue(
      UnicodeText.encodeText("text", value.text),
      value.language.map(UnicodeText.encodeText("text language", _))
    )

  implicit val intoSchema: IntoSchema[Text] =
    new IntoSchema[Text] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.TextType(defaultRestrictions)))
      override def toValue(value: Text): SchemaValue = toSchemaValue(value)
    }

  implicit val fromSchema: FromSchema[Text] =
    new FromSchema[Text] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, Text] =
        value match {
          case SchemaValue.TextValue(text, language) => Right(Text(text, language))
          case other                                 => Left(FromSchemaError(s"expected text value for Text, got $other"))
        }
    }
}

final case class Quantity[U](mantissa: Long, scale: Int, unit: String)

object Quantity {
  private[golem] val SpecConfigKey: String = "golem.schema.quantity.spec"

  private[golem] def encodeSpec(spec: QuantitySpec): String =
    (spec.baseUnit :: spec.allowedSuffixes).mkString("\u001f")

  /**
   * Only the base unit when there are no suffixes, otherwise only the suffixes.
   */
  private[golem] def allows(spec: QuantitySpec, unit: String): Boolean =
    if (spec.allowedSuffixes.isEmpty) unit == spec.baseUnit else spec.allowedSuffixes.contains(unit)

  private[golem] def decodeSpec(value: String): Option[QuantitySpec] =
    value.split("\u001f", -1).toList match {
      case baseUnit :: allowedSuffixes => Some(QuantitySpec(baseUnit, allowedSuffixes, None, None))
      case Nil                         => None
    }

  implicit def schema[U](implicit unit: QuantityUnit[U]): Schema[Quantity[U]] =
    Schema.derived[Quantity[U]].modifier(Modifier.config(SpecConfigKey, encodeSpec(unit.spec)))

  implicit def intoSchema[U](implicit unit: QuantityUnit[U]): IntoSchema[Quantity[U]] =
    new IntoSchema[Quantity[U]] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.QuantityType(unit.spec)))

      override def toValue(value: Quantity[U]): SchemaValue = {
        UnicodeText.encodeText("quantity unit", value.unit)
        if (!unit.allows(value.unit)) throw SchemaEncodeError(s"unit '${value.unit}' is not allowed for quantity")
        SchemaValue.QuantityValueNode(QuantityValue(value.mantissa, value.scale, value.unit))
      }
    }

  def concreteCodec[U](implicit unit: QuantityUnit[U]): wire.ConcreteCodec[Quantity[U]] =
    wire.ConcreteCodec.scalar[Quantity[U]](wire.WitSchemaTypeBody.QuantityType(unit.spec)) { value =>
      UnicodeText.encodeText("quantity unit", value.unit)
      if (!unit.allows(value.unit)) throw SchemaEncodeError(s"unit '${value.unit}' is not allowed for quantity")
      wire.WitSchemaValueNode.QuantityValueNode(QuantityValue(value.mantissa, value.scale, value.unit))
    } { case wire.WitSchemaValueNode.QuantityValueNode(v) =>
      if (!unit.allows(v.unit)) throw SchemaDecodeError(s"unit '${v.unit}' is not allowed for quantity")
      Quantity[U](v.mantissa, v.scale, v.unit)
    }

  implicit def fromSchema[U](implicit unit: QuantityUnit[U]): FromSchema[Quantity[U]] =
    new FromSchema[Quantity[U]] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, Quantity[U]] =
        value match {
          case SchemaValue.QuantityValueNode(v) if unit.allows(v.unit) =>
            Right(Quantity[U](v.mantissa, v.scale, v.unit))
          case SchemaValue.QuantityValueNode(v) =>
            Left(FromSchemaError(s"unit '${v.unit}' is not allowed for quantity"))
          case other => Left(FromSchemaError(s"expected quantity value for Quantity, got $other"))
        }
    }
}

/**
 * The units of the quantity marker `U`, derived by the compiler from the
 * [[golem.runtime.annotations.quantityUnit]] annotation on `U`.
 */
final class QuantityUnit[U] @publicInBinary private[golem] (val baseUnit: String, val allowedSuffixes: List[String]) {
  def spec: QuantitySpec            = QuantitySpec(baseUnit, allowedSuffixes, None, None)
  def allows(unit: String): Boolean = Quantity.allows(spec, unit)
}

object QuantityUnit {
  def apply[U](implicit unit: QuantityUnit[U]): QuantityUnit[U] = unit

  inline given derived[U]: QuantityUnit[U] = ${ QuantityUnitMacro.derive[U] }
}

object RichSchemas {
  private def decodeInstant(v: Datetime): Either[FromSchemaError, Instant] =
    if (v.nanoseconds < 0 || v.nanoseconds >= 1000000000)
      Left(FromSchemaError(s"datetime nanoseconds out of range: ${v.nanoseconds} (expected 0..999999999)"))
    else Right(Instant.ofEpochSecond(v.seconds, v.nanoseconds.toLong))

  implicit val instantSchema: Schema[Instant] =
    Schema[String].transform[Instant](Instant.parse, _.toString)

  implicit val durationSchema: Schema[JDuration] =
    Schema[Long].transform[JDuration](JDuration.ofNanos, _.toNanos)

  implicit val uriSchema: Schema[URI] =
    Schema[String].transform[URI](new URI(_), _.toString)

  implicit val uriIntoSchema: IntoSchema[URI] =
    new IntoSchema[URI] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.UrlType(UrlText.defaultRestrictions)))
      override def toValue(value: URI): SchemaValue = SchemaValue.UrlValue(UrlText.encode(value))
    }

  implicit val uriFromSchema: FromSchema[URI] =
    new FromSchema[URI] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, URI] =
        value match {
          case SchemaValue.UrlValue(v) => UrlText.parse(v).left.map(FromSchemaError(_))
          case other                   => Left(FromSchemaError(s"expected url value for URI, got $other"))
        }
    }

  implicit val instantIntoSchema: IntoSchema[Instant] =
    new IntoSchema[Instant] {
      override lazy val graph: SchemaGraph              = SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.DatetimeType))
      override def toValue(value: Instant): SchemaValue =
        SchemaValue.DatetimeValue(Datetime(value.getEpochSecond, value.getNano))
    }

  implicit val instantFromSchema: FromSchema[Instant] =
    new FromSchema[Instant] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, Instant] =
        value match {
          case SchemaValue.DatetimeValue(v) => decodeInstant(v)
          case other                        => Left(FromSchemaError(s"expected datetime value for Instant, got $other"))
        }
    }

  implicit val durationIntoSchema: IntoSchema[JDuration] =
    new IntoSchema[JDuration] {
      override lazy val graph: SchemaGraph                = SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.DurationType))
      override def toValue(value: JDuration): SchemaValue =
        SchemaValue.DurationValue(DurationNanos.of(value))
    }

  implicit val durationFromSchema: FromSchema[JDuration] =
    new FromSchema[JDuration] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, JDuration] =
        value match {
          case SchemaValue.DurationValue(v) => Right(JDuration.ofNanos(v))
          case other                        => Left(FromSchemaError(s"expected duration value for Duration, got $other"))
        }
    }
}

object Implicits extends RichSchemaImplicits

trait RichSchemaImplicits {
  implicit def golemInstantSchema: Schema[Instant]            = RichSchemas.instantSchema
  implicit def golemDurationSchema: Schema[JDuration]         = RichSchemas.durationSchema
  implicit def golemInstantIntoSchema: IntoSchema[Instant]    = RichSchemas.instantIntoSchema
  implicit def golemInstantFromSchema: FromSchema[Instant]    = RichSchemas.instantFromSchema
  implicit def golemDurationIntoSchema: IntoSchema[JDuration] = RichSchemas.durationIntoSchema
  implicit def golemDurationFromSchema: FromSchema[JDuration] = RichSchemas.durationFromSchema
  implicit def golemUriSchema: Schema[URI]                    = RichSchemas.uriSchema
}
