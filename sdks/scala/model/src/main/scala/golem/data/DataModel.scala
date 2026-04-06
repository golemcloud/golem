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

sealed trait DataType extends Product with Serializable

object DataType {
  final case class Optional(of: DataType) extends DataType

  final case class ListType(of: DataType) extends DataType

  final case class SetType(of: DataType) extends DataType

  final case class MapType(keyType: DataType, valueType: DataType) extends DataType

  final case class TupleType(elements: List[DataType]) extends DataType

  final case class StructType(fields: List[Field], name: Option[String] = None) extends DataType

  final case class EnumType(cases: List[EnumCase], name: Option[String] = None) extends DataType

  final case class PureEnumType(cases: List[String], name: Option[String] = None) extends DataType

  final case class ResultType(ok: Option[DataType], err: Option[DataType]) extends DataType

  final case class Field(name: String, dataType: DataType, optional: Boolean)

  final case class EnumCase(name: String, payload: Option[DataType])

  case object StringType extends DataType

  case object BoolType extends DataType

  case object CharType extends DataType

  case object ByteType extends DataType

  case object ShortType extends DataType

  case object IntType extends DataType

  case object LongType extends DataType

  case object FloatType extends DataType

  case object DoubleType extends DataType

  case object UByteType extends DataType

  case object UShortType extends DataType

  case object UIntType extends DataType

  case object ULongType extends DataType

  case object BigDecimalType extends DataType

  case object UUIDType extends DataType

  case object BytesType extends DataType

  case object UnitType extends DataType
}

sealed trait DataValue extends Product with Serializable

object DataValue {
  final case class StringValue(value: String) extends DataValue

  final case class BoolValue(value: Boolean) extends DataValue

  final case class CharValue(value: Char) extends DataValue

  final case class ByteValue(value: Byte) extends DataValue

  final case class ShortValue(value: Short) extends DataValue

  final case class IntValue(value: Int) extends DataValue

  final case class LongValue(value: Long) extends DataValue

  final case class FloatValue(value: Float) extends DataValue

  final case class DoubleValue(value: Double) extends DataValue

  final case class UByteValue(value: Short) extends DataValue

  final case class UShortValue(value: Int) extends DataValue

  final case class UIntValue(value: Long) extends DataValue

  final case class ULongValue(value: BigInt) extends DataValue

  final case class BigDecimalValue(value: BigDecimal) extends DataValue

  final case class UUIDValue(value: java.util.UUID) extends DataValue

  final case class BytesValue(value: Array[Byte]) extends DataValue

  final case class OptionalValue(value: Option[DataValue]) extends DataValue

  final case class ListValue(values: List[DataValue]) extends DataValue

  final case class SetValue(values: Set[DataValue]) extends DataValue

  final case class MapValue(entries: List[(DataValue, DataValue)]) extends DataValue

  final case class TupleValue(values: List[DataValue]) extends DataValue

  final case class StructValue(fields: Map[String, DataValue]) extends DataValue

  final case class EnumValue(caseName: String, payload: Option[DataValue]) extends DataValue

  final case class PureEnumValue(caseName: String) extends DataValue

  final case class ResultValue(value: Either[DataValue, DataValue]) extends DataValue

  case object NullValue extends DataValue
}
