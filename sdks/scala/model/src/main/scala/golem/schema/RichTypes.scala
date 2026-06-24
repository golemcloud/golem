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
import zio.blocks.typeid.TypeId

import java.time.{Duration => JDuration, Instant}
import scala.collection.immutable.ListMap
import scala.util.control.NonFatal

final case class GolemPath(value: String) extends AnyVal

object GolemPath {
  val defaultSpec: PathSpec = PathSpec(PathDirection.InOut, PathKind.Any)

  implicit val schema: Schema[GolemPath] = Schema.derived

  implicit val intoSchema: IntoSchema[GolemPath] =
    new IntoSchema[GolemPath] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.PathType(defaultSpec)))
      override def toValue(value: GolemPath): SchemaValue = SchemaValue.PathValue(value.value)
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

final case class Url(value: String) extends AnyVal

object Url {
  val defaultRestrictions: UrlRestrictions = UrlRestrictions.empty

  implicit val schema: Schema[Url] = Schema.derived

  implicit val intoSchema: IntoSchema[Url] =
    new IntoSchema[Url] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.UrlType(defaultRestrictions)))
      override def toValue(value: Url): SchemaValue = SchemaValue.UrlValue(value.value)
    }

  implicit val fromSchema: FromSchema[Url] =
    new FromSchema[Url] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, Url] =
        value match {
          case SchemaValue.UrlValue(v) => Right(Url(v))
          case other                   => Left(FromSchemaError(s"expected url value for Url, got $other"))
        }
    }
}

final case class Quantity[U](mantissa: Long, scale: Int, unit: String)

object Quantity {
  private[golem] val SpecConfigKey: String = "golem.schema.quantity.spec"

  private[golem] def encodeSpec(spec: QuantitySpec): String =
    (spec.baseUnit :: spec.allowedSuffixes).mkString("\u001f")

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
        if (!unit.allows(value.unit)) throw SchemaEncodeError(s"unit '${value.unit}' is not allowed for quantity")
        SchemaValue.QuantityValueNode(QuantityValue(value.mantissa, value.scale, value.unit))
      }
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

trait QuantityUnit[U] {
  def baseUnit: String
  def allowedSuffixes: List[String]
  def typeId: TypeId[U]

  final def spec: QuantitySpec            = QuantitySpec(baseUnit, allowedSuffixes, None, None)
  final def allows(unit: String): Boolean = unit == baseUnit || allowedSuffixes.contains(unit)
}

object QuantityUnit {
  def apply[U](implicit unit: QuantityUnit[U]): QuantityUnit[U] = unit
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
        try SchemaValue.DurationValue(value.toNanos)
        catch {
          case NonFatal(e) => throw SchemaEncodeError(s"Duration is out of i64 nanoseconds range: ${e.getMessage}")
        }
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
}
