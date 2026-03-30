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

package golem.runtime.autowire

import golem.data.DataValue._
import golem.data.{DataType, DataValue}
import golem.host.js._
import golem.host.js.JsResult

import scala.scalajs.js

private[golem] object WitValueBuilder {
  def build(dataType: DataType, value: DataValue): Either[String, JsWitValue] = {
    val builder = new Builder
    builder.build(dataType, value).map(_ => builder.result())
  }

  private final class Builder {
    private val nodes = js.Array[JsWitNode]()

    def result(): JsWitValue =
      JsWitValue(nodes)

    def build(dataType: DataType, value: DataValue): Either[String, Int] = {
      val index      = newNode()
      val nodeEither = (dataType, value) match {
        case (DataType.UnitType, NullValue) =>
          Right(JsWitNode.tupleValue(js.Array[JsNodeIndex]()))
        case (DataType.StringType, StringValue(v)) =>
          Right(JsWitNode.primString(v))
        case (DataType.BoolType, BoolValue(v)) =>
          Right(JsWitNode.primBool(v))
        case (DataType.CharType, CharValue(v)) =>
          Right(JsWitNode.primChar(v.toString))
        case (DataType.ByteType, ByteValue(v)) =>
          Right(JsWitNode.primS8(v))
        case (DataType.ShortType, ShortValue(v)) =>
          Right(JsWitNode.primS16(v))
        case (DataType.IntType, IntValue(v)) =>
          Right(JsWitNode.primS32(v))
        case (DataType.LongType, LongValue(v)) =>
          Right(JsWitNode.primS64(js.BigInt(v.toString)))
        case (DataType.FloatType, FloatValue(v)) =>
          Right(JsWitNode.primFloat32(v))
        case (DataType.DoubleType, DoubleValue(v)) =>
          Right(JsWitNode.primFloat64(v))
        case (DataType.UByteType, UByteValue(v)) =>
          Right(JsWitNode.primU8(v))
        case (DataType.UShortType, UShortValue(v)) =>
          Right(JsWitNode.primU16(v))
        case (DataType.UIntType, UIntValue(v)) =>
          Right(JsWitNode.primU32(v.toDouble))
        case (DataType.ULongType, ULongValue(v)) =>
          Right(JsWitNode.primU64(js.BigInt(v.toString)))
        case (DataType.BigDecimalType, BigDecimalValue(v)) =>
          val stringIndex = newNode()
          nodes(stringIndex) = JsWitNode.primString(v.toString)
          Right(JsWitNode.recordValue(js.Array(stringIndex)))
        case (DataType.UUIDType, UUIDValue(v)) =>
          val stringIndex = newNode()
          nodes(stringIndex) = JsWitNode.primString(v.toString)
          Right(JsWitNode.recordValue(js.Array(stringIndex)))
        case (DataType.BytesType, BytesValue(bytes)) =>
          val indicesEither = bytes.toList.foldLeft[Either[String, List[Int]]](Right(Nil)) { case (acc, b) =>
            acc.map { collected =>
              val idx = newNode()
              nodes(idx) = JsWitNode.primU8((b & 0xff).toShort)
              idx :: collected
            }
          }
          indicesEither.map(indices => JsWitNode.listValue(js.Array(indices.reverse: _*)))
        case (DataType.Optional(of), OptionalValue(maybeValue)) =>
          maybeValue match {
            case Some(inner) =>
              build(of, inner).map { child =>
                JsWitNode.optionValue(child: js.UndefOr[JsNodeIndex])
              }
            case None =>
              Right(JsWitNode.optionValue(js.undefined))
          }
        case (DataType.ListType(of), ListValue(values)) =>
          encodeSequence(values, of, "list-value")
        case (DataType.SetType(of), SetValue(values)) =>
          encodeSequence(values.toList, of, "list-value")
        case (DataType.MapType(keyType, valueType), MapValue(entries)) =>
          val entryType   = DataType.TupleType(List(keyType, valueType))
          val entryValues = entries.map { case (k, v) =>
            TupleValue(List(k, v))
          }
          encodeSequence(entryValues, entryType, "list-value")
        case (DataType.TupleType(elements), TupleValue(values)) =>
          if (elements.length != values.length)
            Left(s"Tuple arity mismatch. Expected ${elements.length} values.")
          else
            encodeIndexed(values.zip(elements), "tuple-value")
        case (struct: DataType.StructType, StructValue(fields)) =>
          val orderedEither = struct.fields.foldLeft[Either[String, List[(DataValue, DataType)]]](Right(Nil)) {
            case (acc, field) =>
              acc.flatMap { collected =>
                fields.get(field.name) match {
                  case Some(value) =>
                    Right(collected :+ (value -> field.dataType))
                  case None if field.optional =>
                    Right(collected :+ (NullValue -> field.dataType))
                  case None =>
                    Left(s"Missing required field ${field.name}")
                }
              }
          }
          orderedEither.flatMap(values => encodeIndexed(values, "record-value"))
        case (enumType: DataType.EnumType, EnumValue(caseName, payload)) =>
          val index = enumType.cases.indexWhere(_.name == caseName)
          if (index < 0) Left(s"Unknown enum case $caseName")
          else
            payload match {
              case Some(valuePayload) =>
                build(enumType.cases(index).payload.get, valuePayload).map { child =>
                  JsWitNode.variantValue(index, child: js.UndefOr[JsNodeIndex])
                }
              case None =>
                Right(JsWitNode.variantValue(index, js.undefined))
            }
        case (pureEnum: DataType.PureEnumType, PureEnumValue(caseName)) =>
          val index = pureEnum.cases.indexOf(caseName)
          if (index < 0) Left(s"Unknown pure enum case $caseName")
          else Right(JsWitNode.enumValue(index))
        case (resultType: DataType.ResultType, ResultValue(either)) =>
          either match {
            case Right(okVal) =>
              resultType.ok match {
                case Some(okType) =>
                  build(okType, okVal).map { child =>
                    JsWitNode.resultValue(JsResult.okOptional[JsNodeIndex](child: js.UndefOr[JsNodeIndex]))
                  }
                case None =>
                  Right(JsWitNode.resultValue(JsResult.okOptional[JsNodeIndex](js.undefined)))
              }
            case Left(errVal) =>
              resultType.err match {
                case Some(errType) =>
                  build(errType, errVal).map { child =>
                    JsWitNode.resultValue(JsResult.errOptional[JsNodeIndex](child: js.UndefOr[JsNodeIndex]))
                  }
                case None =>
                  Right(JsWitNode.resultValue(JsResult.errOptional[JsNodeIndex](js.undefined)))
              }
          }
        case other =>
          Left(s"Unsupported value encoding for $other")
      }

      nodeEither.map { node =>
        nodes(index) = node
        index
      }
    }

    private def encodeSequence(
      values: List[DataValue],
      elementType: DataType,
      tag: String
    ): Either[String, JsWitNode] = {
      val indicesEither = values.foldLeft[Either[String, List[Int]]](Right(Nil)) { case (acc, value) =>
        acc.flatMap { collected =>
          build(elementType, value).map(idx => idx :: collected)
        }
      }

      indicesEither.map { indices =>
        val reversed = indices.reverse
        tag match {
          case "list-value"  => JsWitNode.listValue(js.Array(reversed: _*))
          case "tuple-value" => JsWitNode.tupleValue(js.Array(reversed: _*))
          case _             => JsWitNode.listValue(js.Array(reversed: _*))
        }
      }
    }

    private def encodeIndexed(pairs: List[(DataValue, DataType)], tag: String): Either[String, JsWitNode] = {
      val indicesEither = pairs.foldLeft[Either[String, List[Int]]](Right(Nil)) { case (acc, (value, dtype)) =>
        acc.flatMap { collected =>
          build(dtype, value).map(idx => idx :: collected)
        }
      }

      indicesEither.map { indices =>
        val reversed = indices.reverse
        tag match {
          case "record-value" => JsWitNode.recordValue(js.Array(reversed: _*))
          case "tuple-value"  => JsWitNode.tupleValue(js.Array(reversed: _*))
          case _              => JsWitNode.tupleValue(js.Array(reversed: _*))
        }
      }
    }

    private def newNode(): Int = {
      val placeholder = JsShape.tagOnly[JsWitNode]("__placeholder")
      nodes.push(placeholder)
      nodes.length - 1
    }
  }
}
