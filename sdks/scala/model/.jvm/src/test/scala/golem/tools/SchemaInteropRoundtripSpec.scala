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

package golem.tools

import golem.data.{
  DataInterop,
  DataType,
  DataValue,
  ElementSchema,
  ElementValue,
  GolemSchema,
  NamedElementValue,
  StructuredSchema,
  StructuredValue,
  UByte,
  UShort,
  UInt,
  ULong
}
import zio.test._
import zio.blocks.schema.Schema

import java.util.UUID

private sealed trait Status
private object Status {
  case object Ok                        extends Status
  final case class Missing(key: String) extends Status
  implicit val schema: Schema[Status] = Schema.derived
}

object SchemaInteropRoundtripSpec extends ZIOSpecDefault {

  private def roundTrip[A](a: A)(implicit gs: GolemSchema[A]): A = {
    val encoded: StructuredValue = gs.encode(a).fold(err => throw new RuntimeException(err), identity)
    gs.decode(encoded).fold(err => throw new RuntimeException(err), identity)
  }

  def spec = suite("SchemaInteropRoundtripSpec")(
    test("round-trip: struct (case class) + optional field + collections") {
      final case class UserId(value: String)
      object UserId { implicit val schema: Schema[UserId] = Schema.derived }

      final case class Profile(id: UserId, age: Option[Int], tags: Set[String], attrs: Map[String, Int])
      object Profile { implicit val schema: Schema[Profile] = Schema.derived }

      val in  = Profile(UserId("u-1"), Some(42), Set("a", "b"), Map("x" -> 1, "y" -> 2))
      val out = roundTrip(in)
      assertTrue(out == in)
    },
    test("round-trip: enum/variant-style ADT with payload") {
      assertTrue(
        roundTrip[Status](Status.Ok) == Status.Ok,
        roundTrip[Status](Status.Missing("k")) == Status.Missing("k")
      )
    },
    test("round-trip: UUID") {
      final case class Id(value: UUID)
      object Id {
        implicit val schema: Schema[Id] = Schema.derived
      }

      val in  = Id(UUID.fromString("123e4567-e89b-12d3-a456-426614174000"))
      val out = roundTrip(in)
      assertTrue(out == in)
    },
    test("custom conversion example: bytes round-trip via custom GolemSchema") {
      implicit val bytesSchema: GolemSchema[Array[Byte]] = new GolemSchema[Array[Byte]] {
        override val schema: StructuredSchema =
          StructuredSchema.single(ElementSchema.Component(DataType.BytesType))

        override def encode(value: Array[Byte]): Either[String, StructuredValue] =
          Right(StructuredValue.single(ElementValue.Component(DataValue.BytesValue(value))))

        override def decode(value: StructuredValue): Either[String, Array[Byte]] =
          value match {
            case StructuredValue.Tuple(
                  NamedElementValue(_, ElementValue.Component(DataValue.BytesValue(bytes))) :: Nil
                ) =>
              Right(bytes)
            case other =>
              Left(s"Expected component bytes payload, found: $other")
          }
      }

      val in  = Array[Byte](1, 2, 3)
      val out = roundTrip(in)
      assertTrue(out.toSeq == in.toSeq)
    },
    test("round-trip: Byte produces ByteType") {
      final case class ByteWrap(value: Byte)
      object ByteWrap { implicit val schema: Schema[ByteWrap] = Schema.derived }

      val dt = DataInterop.schemaToDataType(implicitly[Schema[ByteWrap]])
      dt match {
        case DataType.StructType(fields, _) =>
          assertTrue(fields.head.dataType == DataType.ByteType)
        case other => throw new RuntimeException(s"Expected StructType, got: $other")
      }
    },
    test("round-trip: Short produces ShortType") {
      final case class ShortWrap(value: Short)
      object ShortWrap { implicit val schema: Schema[ShortWrap] = Schema.derived }

      val dt = DataInterop.schemaToDataType(implicitly[Schema[ShortWrap]])
      dt match {
        case DataType.StructType(fields, _) =>
          assertTrue(fields.head.dataType == DataType.ShortType)
        case other => throw new RuntimeException(s"Expected StructType, got: $other")
      }
    },
    test("round-trip: Float produces FloatType") {
      final case class FloatWrap(value: Float)
      object FloatWrap { implicit val schema: Schema[FloatWrap] = Schema.derived }

      val dt = DataInterop.schemaToDataType(implicitly[Schema[FloatWrap]])
      dt match {
        case DataType.StructType(fields, _) =>
          assertTrue(fields.head.dataType == DataType.FloatType)
        case other => throw new RuntimeException(s"Expected StructType, got: $other")
      }
    },
    test("round-trip: Byte value preserves exact width") {
      val in = roundTrip[Byte](42.toByte)
      assertTrue(in == 42.toByte)
    },
    test("round-trip: Short value preserves exact width") {
      val in = roundTrip[Short](1000.toShort)
      assertTrue(in == 1000.toShort)
    },
    test("round-trip: Float value preserves exact width") {
      val in = roundTrip[Float](3.14f)
      assertTrue(in == 3.14f)
    },
    test("DataType for UByte is UByteType") {
      assertTrue(DataInterop.schemaToDataType(implicitly[Schema[UByte]]) == DataType.UByteType)
    },
    test("DataType for UShort is UShortType") {
      assertTrue(DataInterop.schemaToDataType(implicitly[Schema[UShort]]) == DataType.UShortType)
    },
    test("DataType for UInt is UIntType") {
      assertTrue(DataInterop.schemaToDataType(implicitly[Schema[UInt]]) == DataType.UIntType)
    },
    test("DataType for ULong is ULongType") {
      assertTrue(DataInterop.schemaToDataType(implicitly[Schema[ULong]]) == DataType.ULongType)
    },
    test("round-trip: UByte value preserves unsigned range") {
      val in = roundTrip[UByte](UByte(200))
      assertTrue(in == UByte(200))
    },
    test("round-trip: UShort value preserves unsigned range") {
      val in = roundTrip[UShort](UShort(50000))
      assertTrue(in == UShort(50000))
    },
    test("round-trip: UInt value preserves unsigned range") {
      val in = roundTrip[UInt](UInt(3000000000L))
      assertTrue(in == UInt(3000000000L))
    },
    test("round-trip: ULong value preserves unsigned range") {
      val v  = BigInt("18446744073709551615")
      val in = roundTrip[ULong](ULong(v))
      assertTrue(in == ULong(v))
    },
    test("Schema -> DataType shape is stable for common constructs") {
      final case class Rec(a: String, b: Option[Int], c: List[String])
      object Rec { implicit val schema: Schema[Rec] = Schema.derived }

      val dt = DataInterop.schemaToDataType(implicitly[Schema[Rec]])

      dt match {
        case DataType.StructType(fields, _) =>
          assertTrue(fields.map(_.name) == List("a", "b", "c"))
        case other =>
          throw new RuntimeException(s"Expected StructType, got: $other")
      }
    }
  )
}
