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

package golem.data

import zio.blocks.chunk.Chunk
import zio.blocks.schema.{DynamicValue, PrimitiveValue, Schema}
import zio.test._

object DataInteropReflectionSpec extends ZIOSpecDefault {
  final case class Person(name: String, age: Int, tags: List[String], nickname: Option[String])
  implicit val personSchema: Schema[Person] = Schema.derived

  final case class Tuple2Like(_1: Int, _2: String)
  implicit val tuple2LikeSchema: Schema[Tuple2Like] = Schema.derived

  sealed trait Choice
  case object Yes                     extends Choice
  final case class No(reason: String) extends Choice
  implicit val choiceSchema: Schema[Choice] = Schema.derived

  object Maybe {
    sealed trait Value
    case object None                                   extends Value
    final case class Some(payload: Int, label: String) extends Value
    implicit val schema: Schema[Value] = Schema.derived
  }

  sealed trait ValueChoice
  case object Empty                  extends ValueChoice
  final case class Value(value: Int) extends ValueChoice
  implicit val valueChoiceSchema: Schema[ValueChoice] = Schema.derived

  private def invokeDynamicToDataValue[A](schema: Schema[A], dynamic: DynamicValue): Either[Throwable, DataValue] = {
    val reflectArg = schema.reflect.asInstanceOf[AnyRef]
    val dynamicArg = dynamic.asInstanceOf[AnyRef]
    val method     =
      DataInterop.getClass.getDeclaredMethods.find { m =>
        m.getName.endsWith("dynamicToDataValue") &&
        m.getReturnType != classOf[Option[_]] &&
        m.getParameterCount == 2 &&
        m.getParameterTypes.apply(0).isAssignableFrom(reflectArg.getClass) &&
        m.getParameterTypes.apply(1).isAssignableFrom(dynamicArg.getClass)
      }
        .getOrElse(throw new RuntimeException("dynamicToDataValue method not found"))
    method.setAccessible(true)
    scala.util.Try(method.invoke(DataInterop, reflectArg, dynamicArg).asInstanceOf[DataValue]).toEither
  }

  private def invokeSimpleCaseName(input: String): String = {
    val method =
      DataInterop.getClass.getDeclaredMethods
        .find(_.getName.contains("simpleCaseName"))
        .getOrElse(
          throw new RuntimeException("simpleCaseName method not found")
        )
    method.setAccessible(true)
    method.invoke(DataInterop, input).asInstanceOf[String]
  }

  private def invokeDataValueToDynamic[A](schema: Schema[A], value: DataValue): Either[Throwable, DynamicValue] = {
    val reflectArg = schema.reflect.asInstanceOf[AnyRef]
    val valueArg   = value.asInstanceOf[AnyRef]
    val method     =
      DataInterop.getClass.getDeclaredMethods.find { m =>
        m.getName.endsWith("dataValueToDynamic") &&
        m.getReturnType != classOf[Option[_]] &&
        m.getParameterCount == 2 &&
        m.getParameterTypes.apply(0).isAssignableFrom(reflectArg.getClass) &&
        m.getParameterTypes.apply(1).isAssignableFrom(valueArg.getClass)
      }
        .getOrElse(throw new RuntimeException("dataValueToDynamic method not found"))
    method.setAccessible(true)
    scala.util.Try(method.invoke(DataInterop, reflectArg, valueArg).asInstanceOf[DynamicValue]).toEither
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("DataInteropReflectionSpec")(
      test("dynamicToDataValue rejects invalid dynamic shapes") {
        val intSchema     = Schema[Int]
        val optionSchema  = Schema[Option[Int]]
        val tupleSchema   = Schema[Tuple2Like]
        val recordSchema  = Schema[Person]
        val listSchema    = Schema[List[Int]]
        val mapSchema     = Schema[Map[String, Int]]
        val variantSchema = Schema[Choice]

        val attempts = List(
          invokeDynamicToDataValue(intSchema, DynamicValue.Record(Chunk.empty)),
          invokeDynamicToDataValue(optionSchema, DynamicValue.Primitive(PrimitiveValue.Int(1))),
          invokeDynamicToDataValue(tupleSchema, DynamicValue.Primitive(PrimitiveValue.Int(1))),
          invokeDynamicToDataValue(recordSchema, DynamicValue.Primitive(PrimitiveValue.Int(1))),
          invokeDynamicToDataValue(listSchema, DynamicValue.Record(Chunk.empty)),
          invokeDynamicToDataValue(mapSchema, DynamicValue.Sequence(Chunk.empty)),
          invokeDynamicToDataValue(variantSchema, DynamicValue.Record(Chunk.empty))
        )

        assertTrue(attempts.forall(_.isLeft))
      },
      test("dynamicToDataValue reports specific option/tuple/record failures") {
        val optionSchema = Schema[Option[Int]]
        val tupleSchema  = Schema[Tuple2Like]
        val recordSchema = Schema[Person]

        val missingValue =
          DynamicValue.Variant(
            "Some",
            DynamicValue.Record(Chunk("other" -> DynamicValue.Primitive(PrimitiveValue.Int(1))))
          )
        val nonVariant   = DynamicValue.Primitive(PrimitiveValue.Int(1))
        val tupleNonRec  = DynamicValue.Primitive(PrimitiveValue.String("oops"))
        val recordNonRec = DynamicValue.Primitive(PrimitiveValue.String("oops"))

        assertTrue(invokeDynamicToDataValue(optionSchema, missingValue).isLeft) &&
        assertTrue(invokeDynamicToDataValue(optionSchema, nonVariant).isLeft) &&
        assertTrue(invokeDynamicToDataValue(tupleSchema, tupleNonRec).isLeft) &&
        assertTrue(invokeDynamicToDataValue(recordSchema, recordNonRec).isLeft)
      },
      test("dynamicToDataValue rejects non-primitive for primitive schemas") {
        val intSchema = Schema[Int]
        val bad       = DynamicValue.Record(Chunk.empty)
        assertTrue(invokeDynamicToDataValue(intSchema, bad).isLeft)
      },
      test("dynamicToDataValue handles unit primitive values") {
        val unitSchema = Schema[Unit]
        val dv         = DynamicValue.Primitive(PrimitiveValue.Unit)
        assertTrue(invokeDynamicToDataValue(unitSchema, dv) == Right(DataValue.NullValue))
      },
      test("dynamicToDataValue rejects non-sequence and non-map payloads") {
        val listSchema = Schema[List[Int]]
        val mapSchema  = Schema[Map[String, Int]]

        val nonSeq = DynamicValue.Record(Chunk.empty)
        val nonMap = DynamicValue.Sequence(Chunk.empty)

        assertTrue(invokeDynamicToDataValue(listSchema, nonSeq).isLeft) &&
        assertTrue(invokeDynamicToDataValue(mapSchema, nonMap).isLeft)
      },
      test("dynamicToDataValue supports non-string map keys") {
        val mapSchema = Schema[Map[Int, Int]]
        val intMap    =
          DynamicValue.Map(
            Chunk(
              DynamicValue.Primitive(PrimitiveValue.Int(1)) ->
                DynamicValue.Primitive(PrimitiveValue.Int(2))
            )
          )

        assertTrue(
          invokeDynamicToDataValue(mapSchema, intMap) == Right(
            DataValue.MapValue(List((DataValue.IntValue(1), DataValue.IntValue(2))))
          )
        )
      },
      test("dynamicToDataValue converts option variants") {
        val optionSchema = Schema[Option[Int]]
        val noneDyn      = DynamicValue.Variant("None", DynamicValue.Record(Chunk.empty))
        val someDyn      =
          DynamicValue.Variant(
            "Some",
            DynamicValue.Record(Chunk("value" -> DynamicValue.Primitive(PrimitiveValue.Int(1))))
          )

        assertTrue(invokeDynamicToDataValue(optionSchema, noneDyn) == Right(DataValue.OptionalValue(None))) &&
        assertTrue(
          invokeDynamicToDataValue(optionSchema, someDyn) ==
            Right(DataValue.OptionalValue(Some(DataValue.IntValue(1))))
        )
      },
      test("dynamicToDataValue rejects non-record Some payloads for Option") {
        val optionSchema = Schema[Option[Int]]
        val badSome      = DynamicValue.Variant("Some", DynamicValue.Primitive(PrimitiveValue.Int(1)))
        assertTrue(invokeDynamicToDataValue(optionSchema, badSome).isLeft)
      },
      test("dynamicToDataValue converts custom option-like variants") {
        val maybeSchema = Schema[Maybe.Value]
        val someDyn     =
          DynamicValue.Variant(
            "Some",
            DynamicValue.Record(
              Chunk(
                "payload" -> DynamicValue.Primitive(PrimitiveValue.Int(1)),
                "label"   -> DynamicValue.Primitive(PrimitiveValue.String("x"))
              )
            )
          )

        assertTrue(invokeDynamicToDataValue(maybeSchema, someDyn).isRight)
      },
      test("dynamicToDataValue validates tuple arity") {
        val tupleSchema = Schema[Tuple2Like]
        val badTuple    =
          DynamicValue.Record(
            Chunk(
              "_1" -> DynamicValue.Primitive(PrimitiveValue.Int(1))
            )
          )

        assertTrue(invokeDynamicToDataValue(tupleSchema, badTuple).isLeft)
      },
      test("dynamicToDataValue handles non-record payload for value wrappers") {
        val schema  = Schema[ValueChoice]
        val payload = DynamicValue.Variant("Value", DynamicValue.Primitive(PrimitiveValue.Int(9)))
        assertTrue(invokeDynamicToDataValue(schema, payload).isLeft)
      },
      test("dynamicToDataValue rejects missing value fields in value wrappers") {
        val schema     = Schema[ValueChoice]
        val badPayload =
          DynamicValue.Variant(
            "Value",
            DynamicValue.Record(Chunk("other" -> DynamicValue.Primitive(PrimitiveValue.Int(1))))
          )
        assertTrue(invokeDynamicToDataValue(schema, badPayload).isLeft)
      },
      test("dynamicToDataValue converts map entries") {
        val mapSchema = Schema[Map[String, Int]]
        val okMap     =
          DynamicValue.Map(
            Chunk(
              DynamicValue.Primitive(PrimitiveValue.String("k")) ->
                DynamicValue.Primitive(PrimitiveValue.Int(1))
            )
          )

        assertTrue(
          invokeDynamicToDataValue(mapSchema, okMap) == Right(
            DataValue.MapValue(List((DataValue.StringValue("k"), DataValue.IntValue(1))))
          )
        )
      },
      test("dynamicToDataValue converts sequences to list/set values") {
        val listSchema = Schema[List[Int]]
        val setSchema  = Schema[Set[String]]
        val listDyn    = DynamicValue.Sequence(Chunk(DynamicValue.Primitive(PrimitiveValue.Int(1))))
        val setDyn     = DynamicValue.Sequence(Chunk(DynamicValue.Primitive(PrimitiveValue.String("a"))))

        assertTrue(
          invokeDynamicToDataValue(listSchema, listDyn) == Right(DataValue.ListValue(List(DataValue.IntValue(1)))),
          invokeDynamicToDataValue(setSchema, setDyn) == Right(DataValue.SetValue(Set(DataValue.StringValue("a"))))
        )
      },
      test("dynamicToDataValue converts records and empty variants") {
        val personSchema = Schema[Person]
        val personDyn    =
          DynamicValue.Record(
            Chunk(
              "name"     -> DynamicValue.Primitive(PrimitiveValue.String("x")),
              "age"      -> DynamicValue.Primitive(PrimitiveValue.Int(1)),
              "tags"     -> DynamicValue.Sequence(Chunk(DynamicValue.Primitive(PrimitiveValue.String("t")))),
              "nickname" -> DynamicValue.Variant("None", DynamicValue.Record(Chunk.empty))
            )
          )

        val choiceSchema = Schema[Choice]
        val yesDyn       = DynamicValue.Variant("Yes", DynamicValue.Record(Chunk.empty))

        assertTrue(invokeDynamicToDataValue(personSchema, personDyn).isRight) &&
        assertTrue(invokeDynamicToDataValue(choiceSchema, yesDyn) == Right(DataValue.EnumValue("Yes", None)))
      },
      test("dynamicToDataValue rejects unknown variant cases") {
        val schema  = Schema[Choice]
        val unknown = DynamicValue.Variant("Unknown", DynamicValue.Record(Chunk.empty))
        assertTrue(invokeDynamicToDataValue(schema, unknown).isLeft)
      },
      test("dynamicToDataValue falls back for dynamic schemas") {
        val schema = Schema[DynamicValue]
        val dv     = DynamicValue.Primitive(PrimitiveValue.String("x"))
        assertTrue(invokeDynamicToDataValue(schema, dv).isRight)
      },
      test("simpleCaseName strips prefixes and trailing $") {
        assertTrue(
          invokeSimpleCaseName("scala.None$") == "None",
          invokeSimpleCaseName("Some") == "Some",
          invokeSimpleCaseName("golem.data.Foo") == "Foo"
        )
      },
      test("dataValueToDynamic reports option/tuple/record errors") {
        val optionSchema = Schema[Option[Int]]
        val tupleSchema  = Schema[Tuple2Like]
        val recordSchema = Schema[Person]

        assertTrue(invokeDataValueToDynamic(optionSchema, DataValue.StringValue("oops")).isLeft) &&
        assertTrue(
          invokeDataValueToDynamic(
            tupleSchema,
            DataValue.TupleValue(List(DataValue.IntValue(1), DataValue.IntValue(2), DataValue.IntValue(3)))
          ).isLeft
        ) &&
        assertTrue(invokeDataValueToDynamic(tupleSchema, DataValue.StringValue("oops")).isLeft) &&
        assertTrue(invokeDataValueToDynamic(recordSchema, DataValue.StringValue("oops")).isLeft)
      },
      test("dataValueToDynamic rejects invalid primitives and records") {
        val intSchema    = Schema[Int]
        val personSchema = Schema[Person]

        assertTrue(invokeDataValueToDynamic(intSchema, DataValue.BytesValue(Array[Byte](1, 2))).isLeft) &&
        assertTrue(invokeDataValueToDynamic(intSchema, DataValue.StructValue(Map.empty)).isLeft) &&
        assertTrue(invokeDataValueToDynamic(personSchema, DataValue.StructValue(Map.empty)).isLeft)
      },
      test("dataValueToDynamic rejects invalid sequences and maps") {
        val listSchema = Schema[List[Int]]
        val mapSchema  = Schema[Map[String, Int]]

        assertTrue(invokeDataValueToDynamic(listSchema, DataValue.StringValue("oops")).isLeft) &&
        assertTrue(invokeDataValueToDynamic(mapSchema, DataValue.StringValue("oops")).isLeft)
      },
      test("dataValueToDynamic handles variants with and without payloads") {
        val choiceSchema = Schema[Choice]
        val valueSchema  = Schema[ValueChoice]

        val noneVariant = DataValue.EnumValue("Yes", None)
        val someVariant = DataValue.EnumValue("Value", Some(DataValue.IntValue(1)))

        assertTrue(invokeDataValueToDynamic(choiceSchema, noneVariant).isRight) &&
        assertTrue(invokeDataValueToDynamic(valueSchema, someVariant).isRight) &&
        assertTrue(invokeDataValueToDynamic(valueSchema, DataValue.StringValue("oops")).isLeft)
      },
      test("dataValueToDynamic uses dynamic fallback for unhandled schemas") {
        val schema = Schema[DynamicValue]
        val value  = DataValue.StringValue("x")
        assertTrue(
          invokeDataValueToDynamic(schema, value) ==
            Right(DynamicValue.Primitive(PrimitiveValue.String("StringValue(x)")))
        )
      }
    )
}
