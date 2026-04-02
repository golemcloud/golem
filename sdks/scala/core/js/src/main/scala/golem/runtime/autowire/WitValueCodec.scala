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

import golem.data.DataType._
import golem.data.DataValue._
import golem.data.{DataType, DataValue}
import golem.host.js._
import golem.host.js.{JsOk, JsErr}

import scala.scalajs.js

private[golem] object WitValueCodec {
  def decode(dataType: DataType, witValue: JsWitValue): Either[String, DataValue] = {
    val nodes = witValue.nodes
    decodeNode(dataType, nodes, 0)
  }

  private def decodeNode(dataType: DataType, nodes: js.Array[JsWitNode], index: Int): Either[String, DataValue] = {
    if (index < 0 || index >= nodes.length)
      Left(s"Wit node index $index out of bounds")
    else {
      val node = nodes(index)
      val tag  = node.tag

      (dataType, tag) match {
        case (UnitType, "tuple-value") =>
          Right(NullValue)
        case (StringType, "prim-string") =>
          Right(StringValue(node.asInstanceOf[JsWitNodePrimString].value))
        case (BoolType, "prim-bool") =>
          Right(BoolValue(node.asInstanceOf[JsWitNodePrimBool].value))
        case (CharType, "prim-char") =>
          Right(CharValue(node.asInstanceOf[JsWitNodePrimChar].value.charAt(0)))
        case (ByteType, "prim-s8") =>
          Right(ByteValue(node.asInstanceOf[JsWitNodePrimS8].value))
        case (ShortType, "prim-s16") =>
          Right(ShortValue(node.asInstanceOf[JsWitNodePrimS16].value))
        case (IntType, "prim-s32") =>
          Right(IntValue(node.asInstanceOf[JsWitNodePrimS32].value))
        case (IntType, "prim-float64") =>
          val raw = node.asInstanceOf[JsWitNodePrimFloat64].value
          if (!isWholeNumber(raw))
            Left(s"Non-integral numeric value $raw cannot be decoded as IntType")
          else if (!inIntRange(raw))
            Left(s"Value $raw is out of Int range for IntType")
          else
            Right(IntValue(raw.toInt))
        case (LongType, "prim-s64") =>
          val rawVal  = node.asInstanceOf[JsWitNodePrimS64].value
          val longVal = BigInt(rawVal.toString).toLong
          Right(LongValue(longVal))
        case (LongType, "prim-float64") =>
          val raw = node.asInstanceOf[JsWitNodePrimFloat64].value
          if (!isWholeNumber(raw))
            Left(s"Non-integral numeric value $raw cannot be decoded as LongType (prim-float64)")
          else if (!inLongRange(raw))
            Left(s"Value $raw is out of Long range for LongType (prim-float64)")
          else
            Right(LongValue(raw.toLong))
        case (FloatType, "prim-float32") =>
          Right(FloatValue(node.asInstanceOf[JsWitNodePrimFloat32].value))
        case (DoubleType, "prim-float64") =>
          Right(DoubleValue(node.asInstanceOf[JsWitNodePrimFloat64].value))
        case (UByteType, "prim-u8") =>
          Right(UByteValue(node.asInstanceOf[JsWitNodePrimU8].value))
        case (UShortType, "prim-u16") =>
          Right(UShortValue(node.asInstanceOf[JsWitNodePrimU16].value))
        case (UIntType, "prim-u32") =>
          Right(UIntValue(node.asInstanceOf[JsWitNodePrimU32].value.toLong))
        case (ULongType, "prim-u64") =>
          Right(ULongValue(BigInt(node.asInstanceOf[JsWitNodePrimU64].value.toString)))
        case (BigDecimalType, "record-value") =>
          val refs = node.asInstanceOf[JsWitNodeRecordValue].value
          if (refs.length != 1) Left(s"BigDecimal record expected 1 field, found ${refs.length}")
          else
            decodeNode(StringType, nodes, refs(0)).flatMap {
              case StringValue(s) => Right(BigDecimalValue(BigDecimal(s)))
              case other          => Left(s"Expected string inside BigDecimal record, found $other")
            }
        case (UUIDType, "record-value") =>
          val refs = node.asInstanceOf[JsWitNodeRecordValue].value
          if (refs.length != 1) Left(s"UUID record expected 1 field, found ${refs.length}")
          else
            decodeNode(StringType, nodes, refs(0)).flatMap {
              case StringValue(s) => Right(UUIDValue(java.util.UUID.fromString(s)))
              case other          => Left(s"Expected string inside UUID record, found $other")
            }
        case (BytesType, "list-value") =>
          val refs        = node.asInstanceOf[JsWitNodeListValue].value
          val bytesEither = refs.foldLeft[Either[String, Vector[Byte]]](Right(Vector.empty)) { case (acc, childIdx) =>
            for {
              vec  <- acc
              byte <- {
                val child    = nodes(childIdx)
                val childTag = child.tag
                childTag match {
                  case "prim-u8" => Right((child.asInstanceOf[JsWitNodePrimU8].value & 0xff).toByte)
                  case other     => Left(s"Expected prim-u8 byte node, found $other")
                }
              }
            } yield vec :+ byte
          }
          bytesEither.map(vector => BytesValue(vector.toArray))
        case (Optional(of), "option-value") =>
          val ref = node.asInstanceOf[JsWitNodeOptionValue].value
          if (ref.isEmpty) Right(OptionalValue(None))
          else
            decodeNode(of, nodes, ref.get).map(value => OptionalValue(Some(value)))
        case (ListType(of), "list-value") =>
          decodeIndexed(of, nodes, node).map(values => ListValue(values))
        case (SetType(of), "list-value") =>
          decodeIndexed(of, nodes, node).map(values => SetValue(values.toSet))
        case (MapType(keyType, valueType), "list-value") =>
          val entryType = TupleType(List(keyType, valueType))
          decodeIndexed(entryType, nodes, node).flatMap { entries =>
            val pairs = entries.map {
              case TupleValue(List(key, value)) => Right((key, value))
              case other                        => Left(s"Invalid map entry payload: $other")
            }
            pairs.foldLeft[Either[String, List[(DataValue, DataValue)]]](Right(Nil)) {
              case (acc, Right(pair)) => acc.map(_ :+ pair)
              case (_, Left(err))     => Left(err)
            }
          }.map(MapValue(_))
        case (TupleType(elements), "tuple-value") =>
          decodeTuple(elements, nodes, node).map(TupleValue(_))
        case (struct: StructType, "record-value") =>
          val refs = node.asInstanceOf[JsWitNodeRecordValue].value
          if (refs.length != struct.fields.length)
            Left(s"Struct field count mismatch. Expected ${struct.fields.length}, found ${refs.length}")
          else {
            val decoded =
              struct.fields.zipWithIndex.foldLeft[Either[String, Map[String, DataValue]]](Right(Map.empty)) {
                case (acc, (field, idx)) =>
                  acc.flatMap { map =>
                    decodeNode(field.dataType, nodes, refs(idx)).map(value => map.updated(field.name, value))
                  }
              }
            decoded.map(StructValue(_))
          }
        case (enumType: EnumType, "variant-value") =>
          val variantVal = node.asInstanceOf[JsWitNodeVariantValue].value
          val caseIndex  = variantVal._1
          val maybeValue = variantVal._2
          if (caseIndex < 0 || caseIndex >= enumType.cases.length)
            Left(s"Variant index $caseIndex out of range")
          else {
            val selected = enumType.cases(caseIndex)
            if (maybeValue.isEmpty)
              Right(EnumValue(selected.name, None))
            else
              selected.payload match {
                case Some(payloadType) =>
                  decodeNode(payloadType, nodes, maybeValue.get)
                    .map(value => EnumValue(selected.name, Some(value)))
                case None =>
                  Left(s"Variant ${selected.name} does not expect payload")
              }
          }
        case (enumType: EnumType, "enum-value") =>
          val caseIndex = node.asInstanceOf[JsWitNodeEnumValue].value
          if (caseIndex < 0 || caseIndex >= enumType.cases.length)
            Left(s"Enum index $caseIndex out of range")
          else
            Right(EnumValue(enumType.cases(caseIndex).name, None))
        case (pureEnum: PureEnumType, "enum-value") =>
          val caseIndex = node.asInstanceOf[JsWitNodeEnumValue].value
          if (caseIndex < 0 || caseIndex >= pureEnum.cases.length)
            Left(s"Enum index $caseIndex out of range")
          else
            Right(PureEnumValue(pureEnum.cases(caseIndex)))
        case (resultType: ResultType, "result-value") =>
          val resultVal = node.asInstanceOf[JsWitNodeResultValue].value
          val resultTag = resultVal.tag
          resultTag match {
            case "ok" =>
              val okRef = resultVal.asInstanceOf[JsOk[js.UndefOr[JsNodeIndex]]].value
              if (okRef.isEmpty)
                Right(ResultValue(Right(NullValue)))
              else
                resultType.ok match {
                  case Some(okType) =>
                    decodeNode(okType, nodes, okRef.get).map(v => ResultValue(Right(v)))
                  case None =>
                    Left("Result ok has payload but type declares none")
                }
            case "err" =>
              val errRef = resultVal.asInstanceOf[JsErr[js.UndefOr[JsNodeIndex]]].value
              if (errRef.isEmpty)
                Right(ResultValue(Left(NullValue)))
              else
                resultType.err match {
                  case Some(errType) =>
                    decodeNode(errType, nodes, errRef.get).map(v => ResultValue(Left(v)))
                  case None =>
                    Left("Result err has payload but type declares none")
                }
            case other =>
              Left(s"Unknown result tag: $other")
          }
        case other =>
          Left(s"Unsupported decoding for $other with node tag $tag")
      }
    }
  }

  private def decodeIndexed(
    dataType: DataType,
    nodes: js.Array[JsWitNode],
    node: JsWitNode
  ): Either[String, List[DataValue]] = {
    val refs = node.asInstanceOf[JsWitNodeListValue].value
    refs.foldLeft[Either[String, List[DataValue]]](Right(Nil)) { case (acc, childIdx) =>
      acc.flatMap { values =>
        decodeNode(dataType, nodes, childIdx).map(value => values :+ value)
      }
    }
  }

  private def isWholeNumber(raw: Double): Boolean =
    !raw.isNaN && !raw.isInfinity && raw.isWhole

  private def inIntRange(raw: Double): Boolean =
    raw >= Int.MinValue.toDouble && raw <= Int.MaxValue.toDouble

  private def inLongRange(raw: Double): Boolean =
    raw >= Long.MinValue.toDouble && raw <= Long.MaxValue.toDouble

  private def decodeTuple(
    elements: List[DataType],
    nodes: js.Array[JsWitNode],
    node: JsWitNode
  ): Either[String, List[DataValue]] = {
    val refs = node.asInstanceOf[JsWitNodeTupleValue].value
    if (refs.length != elements.length)
      Left(s"Tuple size mismatch. Expected ${elements.length}, found ${refs.length}")
    else {
      val initial: Either[String, List[DataValue]] = Right(Nil)
      refs.zip(elements).foldLeft(initial) { case (acc, (ref, dtype)) =>
        acc.flatMap { values =>
          decodeNode(dtype, nodes, ref).map(value => values :+ value)
        }
      }
    }
  }
}
