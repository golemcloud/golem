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

import zio.blocks.schema.{DynamicValue, Schema}
import zio.test._
import zio.test.Assertion._

object DataInteropSpec extends ZIOSpecDefault {
  final case class Person(name: String, age: Int, tags: List[String], nickname: Option[String])
  implicit val personSchema: Schema[Person] = Schema.derived

  final case class Bag(values: Map[String, Int], labels: Set[String])
  implicit val bagSchema: Schema[Bag] = Schema.derived

  final case class Tuple2Like(_1: Int, _2: String)
  implicit val tuple2LikeSchema: Schema[Tuple2Like] = Schema.derived

  final case class Tuple3Like(_1: Int, _2: String, _3: Boolean)
  implicit val tuple3LikeSchema: Schema[Tuple3Like] = Schema.derived

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

  final case class Primitives(
    str: String,
    bool: Boolean,
    int: Int,
    long: Long,
    double: Double,
    big: BigDecimal,
    uuid: java.util.UUID
  )
  implicit val primitivesSchema: Schema[Primitives] = Schema.derived

  sealed trait Color
  case object Red   extends Color
  case object Green extends Color
  case object Blue  extends Color
  implicit val colorSchema: Schema[Color] = Schema.derived

  final case class NarrowPrimitives(byte: Byte, short: Short, float: Float)
  implicit val narrowPrimitivesSchema: Schema[NarrowPrimitives] = Schema.derived

  final case class Collections(
    list: List[Int],
    set: Set[String],
    map: Map[String, Long],
    opt: Option[Double]
  )
  implicit val collectionsSchema: Schema[Collections] = Schema.derived

  final case class UserId(value: Long)
  implicit val userIdSchema: Schema[UserId] =
    Schema[Long].transform(UserId(_), (userId: UserId) => userId.value)

  override def spec: Spec[TestEnvironment, Any] =
    suite("DataInteropSpec")(
      test("round trips records with options and lists") {
        val value   = Person("Ada", 37, List("math", "code"), Some("ada"))
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[Person](encoded))(isRight(equalTo(value)))
      },
      test("round trips records with None options") {
        val value   = Person("Bob", 12, Nil, None)
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[Person](encoded))(isRight(equalTo(value)))
      },
      test("round trips maps and sets") {
        val value   = Bag(Map("a" -> 1, "b" -> 2), Set("x", "y"))
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[Bag](encoded))(isRight(equalTo(value)))
      },
      test("round trips enum-style values") {
        val yesEncoded = DataInterop.toData[Choice](Yes)
        val noEncoded  = DataInterop.toData[Choice](No("nope"))

        assert(DataInterop.fromData[Choice](yesEncoded))(isRight(equalTo(Yes))) &&
        assert(DataInterop.fromData[Choice](noEncoded))(isRight(equalTo(No("nope"))))
      },
      test("round trips option-like variants with custom payloads") {
        val value: Maybe.Value = Maybe.Some(1, "label")
        val encoded            = DataInterop.toData[Maybe.Value](value)
        assert(DataInterop.fromData[Maybe.Value](encoded))(isRight(equalTo(value)))
      },
      test("round trips value-wrapper variants") {
        val value: ValueChoice = Value(42)
        val encoded            = DataInterop.toData[ValueChoice](value)
        assert(DataInterop.fromData[ValueChoice](encoded))(isRight(equalTo(value)))
      },
      test("round trips primitive fields") {
        val value = Primitives(
          str = "ok",
          bool = true,
          int = 3,
          long = 4L,
          double = 2.5,
          big = BigDecimal("12.34"),
          uuid = java.util.UUID.fromString("123e4567-e89b-12d3-a456-426614174000")
        )
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[Primitives](encoded))(isRight(equalTo(value)))
      },
      test("round trips narrow primitive fields via numeric widening") {
        val value   = NarrowPrimitives(1, 2, 1.25f)
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[NarrowPrimitives](encoded))(isRight(equalTo(value)))
      },
      test("round trips collection-heavy records") {
        val value = Collections(
          list = List(1, 2, 3),
          set = Set("a", "b"),
          map = Map("x" -> 1L, "y" -> 2L),
          opt = Some(1.5)
        )
        val encoded = DataInterop.toData(value)
        assert(DataInterop.fromData[Collections](encoded))(isRight(equalTo(value)))
      },
      test("round trips tuple-like records") {
        val tuple2 = Tuple2Like(1, "one")
        val tuple3 = Tuple3Like(2, "two", true)

        assert(DataInterop.fromData[Tuple2Like](DataInterop.toData(tuple2)))(isRight(equalTo(tuple2))) &&
        assert(DataInterop.fromData[Tuple3Like](DataInterop.toData(tuple3)))(isRight(equalTo(tuple3)))
      },
      test("round trips wrapper schemas") {
        val id = UserId(42L)
        assert(DataInterop.fromData[UserId](DataInterop.toData(id)))(isRight(equalTo(id)))
      },
      test("detects tuple schemas as tuple data types") {
        val tupleType = DataInterop.schemaToDataType(Schema[Tuple2Like])
        assertTrue(tupleType.isInstanceOf[DataType.TupleType]) &&
        assertTrue(tupleType.asInstanceOf[DataType.TupleType].elements.length == 2)
      },
      test("detects tuple3 schemas as tuple data types") {
        val tupleType = DataInterop.schemaToDataType(Schema[Tuple3Like])
        assertTrue(tupleType.isInstanceOf[DataType.TupleType]) &&
        assertTrue(tupleType.asInstanceOf[DataType.TupleType].elements.length == 3)
      },
      test("maps options and sets to optional/set data types") {
        val optType = DataInterop.schemaToDataType(Schema[Option[Int]])
        val setType = DataInterop.schemaToDataType(Schema[Set[String]])

        assertTrue(optType.isInstanceOf[DataType.Optional]) &&
        assertTrue(setType.isInstanceOf[DataType.SetType])
      },
      test("maps primitive schemas to data types") {
        val byteType  = DataInterop.schemaToDataType(Schema[Byte])
        val shortType = DataInterop.schemaToDataType(Schema[Short])
        val floatType = DataInterop.schemaToDataType(Schema[Float])
        val unitType  = DataInterop.schemaToDataType(Schema[Unit])
        val strType   = DataInterop.schemaToDataType(Schema[String])
        val boolType  = DataInterop.schemaToDataType(Schema[Boolean])
        val intType   = DataInterop.schemaToDataType(Schema[Int])
        val longType  = DataInterop.schemaToDataType(Schema[Long])
        val dblType   = DataInterop.schemaToDataType(Schema[Double])
        val bigType   = DataInterop.schemaToDataType(Schema[BigDecimal])
        val uuidType  = DataInterop.schemaToDataType(Schema[java.util.UUID])

        assertTrue(byteType == DataType.ByteType) &&
        assertTrue(shortType == DataType.ShortType) &&
        assertTrue(floatType == DataType.FloatType) &&
        assertTrue(unitType == DataType.UnitType) &&
        assertTrue(strType == DataType.StringType) &&
        assertTrue(boolType == DataType.BoolType) &&
        assertTrue(intType == DataType.IntType) &&
        assertTrue(longType == DataType.LongType) &&
        assertTrue(dblType == DataType.DoubleType) &&
        assertTrue(bigType == DataType.BigDecimalType) &&
        assertTrue(uuidType == DataType.UUIDType)
      },
      test("maps string-key maps to map data types") {
        val mapType = DataInterop.schemaToDataType(Schema[Map[String, Int]])
        assertTrue(mapType.isInstanceOf[DataType.MapType])
      },
      test("maps variant payload shapes to enum cases") {
        sealed trait Payload
        case object EmptyPayload                          extends Payload
        final case class ValuePayload(value: Int)         extends Payload
        final case class RecordPayload(a: Int, b: String) extends Payload

        implicit val payloadSchema: Schema[Payload] = Schema.derived

        val dt = DataInterop.schemaToDataType(Schema[Payload])
        assertTrue(dt.isInstanceOf[DataType.EnumType]) &&
        assertTrue {
          val enumType = dt.asInstanceOf[DataType.EnumType]
          val caseMap  = enumType.cases.map(c => c.name -> c.payload).toMap

          caseMap.get("EmptyPayload").exists(_.isEmpty) &&
          caseMap.get("ValuePayload").exists(_.contains(DataType.IntType)) &&
          caseMap.get("RecordPayload").exists(_.exists(_.isInstanceOf[DataType.StructType]))
        }
      },
      test("wrapper schemas map to underlying data types") {
        val dt = DataInterop.schemaToDataType(Schema[UserId])
        assertTrue(dt == DataType.LongType)
      },
      test("supports arbitrary map key types") {
        val dt = DataInterop.schemaToDataType(Schema[Map[Int, String]])
        assertTrue(dt == DataType.MapType(DataType.IntType, DataType.StringType))
      },
      test("rejects invalid data values for option and tuple schemas") {
        val optionAttempt = scala.util.Try(DataInterop.fromData[Option[Int]](DataValue.StringValue("oops")))
        val tupleAttempt  =
          scala.util.Try(DataInterop.fromData[Tuple2Like](DataValue.TupleValue(List(DataValue.IntValue(1)))))

        assertTrue(optionAttempt.isFailure) &&
        assertTrue(tupleAttempt.isFailure)
      },
      test("rejects tuple arity mismatches and tuple-as-record values") {
        val arityAttempt =
          scala.util.Try(
            DataInterop.fromData[Tuple2Like](
              DataValue.TupleValue(List(DataValue.IntValue(1), DataValue.IntValue(2), DataValue.IntValue(3)))
            )
          )
        val recordAttempt =
          scala.util.Try(
            DataInterop.fromData[Tuple2Like](
              DataValue.StructValue(Map("_1" -> DataValue.IntValue(1), "_2" -> DataValue.StringValue("x")))
            )
          )

        assertTrue(arityAttempt.isFailure) &&
        assertTrue(recordAttempt.isFailure)
      },
      test("char primitive schema and value") {
        val charType  = DataInterop.schemaToDataType(Schema[Char])
        val charValue = DataInterop.toData[Char]('a')

        assertTrue(charType == DataType.CharType) &&
        assertTrue(charValue == DataValue.CharValue('a'))
      },
      test("BigInt maps to BigDecimalType") {
        val bigIntType  = DataInterop.schemaToDataType(Schema[BigInt])
        val bigIntValue = DataInterop.toData[BigInt](BigInt(1))
        assertTrue(bigIntType == DataType.BigDecimalType) &&
        assertTrue(bigIntValue == DataValue.BigDecimalValue(BigDecimal(1)))
      },
      test("encodes non-string map keys") {
        val data = DataInterop.toData(Map(1 -> "a"))(Schema[Map[Int, String]])
        assertTrue(data == DataValue.MapValue(List((DataValue.IntValue(1), DataValue.StringValue("a")))))
      },
      test("rejects invalid data values for record, map, and variant schemas") {
        val recordAttempt =
          scala.util.Try(DataInterop.fromData[Person](DataValue.StructValue(Map("name" -> DataValue.StringValue("x")))))
        val mapAttempt =
          scala.util.Try(DataInterop.fromData[Map[String, Int]](DataValue.ListValue(List(DataValue.IntValue(1)))))
        val variantAttempt =
          scala.util.Try(DataInterop.fromData[Choice](DataValue.StructValue(Map("x" -> DataValue.IntValue(1)))))

        assertTrue(recordAttempt.isFailure) &&
        assertTrue(mapAttempt.isFailure) &&
        assertTrue(variantAttempt.isFailure)
      },
      test("rejects unsupported primitive data values") {
        val attempt = scala.util.Try(DataInterop.fromData[String](DataValue.BytesValue(Array[Byte](1, 2))))
        assertTrue(attempt.isFailure)
      },
      test("rejects non-primitive data values for primitive schemas") {
        val attempt = scala.util.Try(DataInterop.fromData[Int](DataValue.StructValue(Map.empty)))
        assertTrue(attempt.isFailure)
      },
      test("decodes unit from null value") {
        assert(DataInterop.fromData[Unit](DataValue.NullValue))(isRight(equalTo(())))
      },
      test("decodes optional and enum data values") {
        val none  = DataInterop.fromData[Option[Int]](DataValue.OptionalValue(None))
        val some  = DataInterop.fromData[Option[Int]](DataValue.OptionalValue(Some(DataValue.IntValue(1))))
        val yes   = DataInterop.fromData[Choice](DataValue.EnumValue("Yes", None))
        val value = DataInterop.fromData[ValueChoice](DataValue.EnumValue("Value", Some(DataValue.IntValue(2))))

        assert(none)(isRight(equalTo(None))) &&
        assert(some)(isRight(equalTo(Some(1)))) &&
        assert(yes)(isRight(equalTo(Yes))) &&
        assert(value)(isRight(equalTo(Value(2))))
      },
      test("rejects unknown enum cases and record/tuple mismatches") {
        val unknownVariant = DataInterop.fromData[Choice](DataValue.EnumValue("Unknown", None))
        val tupleAsRecord  =
          scala.util.Try(DataInterop.fromData[Person](DataValue.TupleValue(List(DataValue.IntValue(1)))))

        assert(unknownVariant)(isLeft) &&
        assertTrue(tupleAsRecord.isFailure)
      },
      test("decodes list and set data values") {
        val list =
          DataInterop.fromData[List[Int]](DataValue.ListValue(List(DataValue.IntValue(1), DataValue.IntValue(2))))
        val set = DataInterop.fromData[Set[String]](DataValue.SetValue(Set(DataValue.StringValue("a"))))

        assert(list)(isRight(equalTo(List(1, 2)))) &&
        assert(set)(isRight(equalTo(Set("a"))))
      },
      test("encodes dynamic schemas as empty structs") {
        val dt = DataInterop.schemaToDataType(Schema[DynamicValue])
        assertTrue(dt == DataType.StructType(Nil))
      },
      test("exposes derived data types") {
        val dt = DataInterop.dataTypeOf[Person]
        assertTrue(dt.isInstanceOf[DataType.StructType])
      },
      test("maps all-unit sealed trait to PureEnumType") {
        val dt = DataInterop.schemaToDataType(Schema[Color])
        assertTrue(dt == DataType.PureEnumType(List("Red", "Green", "Blue"), name = Some("Color")))
      },
      test("round trips pure enum values") {
        val red: Color   = Red
        val green: Color = Green
        val encoded      = DataInterop.toData(red)
        assertTrue(encoded == DataValue.PureEnumValue("Red")) &&
        assert(DataInterop.fromData[Color](encoded))(isRight(equalTo(Red))) &&
        assert(DataInterop.fromData[Color](DataInterop.toData(green)))(isRight(equalTo(Green)))
      },
      test("maps mixed sealed trait (with payloads) to EnumType") {
        val dt = DataInterop.schemaToDataType(Schema[Choice])
        assertTrue(dt.isInstanceOf[DataType.EnumType])
      }
    )
}
