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

import golem.data.DataType
import golem.host.js._

import scala.scalajs.js

private[golem] object WitTypeBuilder {
  def build(dataType: DataType): JsWitType = {
    val builder = new Builder
    builder.buildNode(dataType)
    builder.result()
  }

  private final class Builder {
    private val nodes = js.Array[JsNamedWitTypeNode]()

    def result(): JsWitType =
      JsWitType(nodes)

    def buildNode(dataType: DataType): Int = {
      val index                   = newNode()
      val (typeVariant, typeName) = dataType match {
        case DataType.UnitType =>
          (JsWitTypeNode.tupleType(js.Array[JsNodeIndex]()), js.undefined: js.UndefOr[String])
        case DataType.StringType =>
          (JsWitTypeNode.primStringType, js.undefined: js.UndefOr[String])
        case DataType.BoolType =>
          (JsWitTypeNode.primBoolType, js.undefined: js.UndefOr[String])
        case DataType.CharType =>
          (JsWitTypeNode.primCharType, js.undefined: js.UndefOr[String])
        case DataType.ByteType =>
          (JsWitTypeNode.primS8Type, js.undefined: js.UndefOr[String])
        case DataType.ShortType =>
          (JsWitTypeNode.primS16Type, js.undefined: js.UndefOr[String])
        case DataType.IntType =>
          (JsWitTypeNode.primS32Type, js.undefined: js.UndefOr[String])
        case DataType.LongType =>
          (JsWitTypeNode.primS64Type, js.undefined: js.UndefOr[String])
        case DataType.FloatType =>
          (JsWitTypeNode.primF32Type, js.undefined: js.UndefOr[String])
        case DataType.DoubleType =>
          (JsWitTypeNode.primF64Type, js.undefined: js.UndefOr[String])
        case DataType.UByteType =>
          (JsWitTypeNode.primU8Type, js.undefined: js.UndefOr[String])
        case DataType.UShortType =>
          (JsWitTypeNode.primU16Type, js.undefined: js.UndefOr[String])
        case DataType.UIntType =>
          (JsWitTypeNode.primU32Type, js.undefined: js.UndefOr[String])
        case DataType.ULongType =>
          (JsWitTypeNode.primU64Type, js.undefined: js.UndefOr[String])
        case DataType.BigDecimalType =>
          val stringIndex = buildNode(DataType.StringType)
          (JsWitTypeNode.recordType(js.Array(js.Tuple2("value", stringIndex))), js.undefined: js.UndefOr[String])
        case DataType.UUIDType =>
          val stringIndex = buildNode(DataType.StringType)
          (JsWitTypeNode.recordType(js.Array(js.Tuple2("value", stringIndex))), js.undefined: js.UndefOr[String])
        case DataType.BytesType =>
          val u8Index = newNode()
          nodes(u8Index) = JsNamedWitTypeNode(JsWitTypeNode.primU8Type)
          (JsWitTypeNode.listType(u8Index), js.undefined: js.UndefOr[String])
        case DataType.Optional(of) =>
          (JsWitTypeNode.optionType(buildNode(of)), js.undefined: js.UndefOr[String])
        case DataType.ListType(of) =>
          (JsWitTypeNode.listType(buildNode(of)), js.undefined: js.UndefOr[String])
        case DataType.SetType(of) =>
          (JsWitTypeNode.listType(buildNode(of)), js.undefined: js.UndefOr[String])
        case DataType.MapType(keyType, valueType) =>
          val entryTuple = DataType.TupleType(List(keyType, valueType))
          val entryIndex = buildNode(entryTuple)
          (JsWitTypeNode.listType(entryIndex), js.undefined: js.UndefOr[String])
        case DataType.TupleType(elements) =>
          (JsWitTypeNode.tupleType(js.Array(elements.map(buildNode): _*)), js.undefined: js.UndefOr[String])
        case DataType.StructType(fields, name) =>
          val fieldEntries = js.Array[js.Tuple2[String, JsNodeIndex]]()
          fields.foreach { field =>
            val idx = buildNode(field.dataType)
            fieldEntries.push(js.Tuple2(field.name, idx))
          }
          (JsWitTypeNode.recordType(fieldEntries), name.fold[js.UndefOr[String]](js.undefined)(identity))
        case DataType.EnumType(cases, name) =>
          val variantEntries = js.Array[js.Tuple2[String, js.UndefOr[JsNodeIndex]]]()
          cases.foreach { enumCase =>
            val payloadIndex = enumCase.payload.map(buildNode)
            variantEntries.push(
              js.Tuple2(enumCase.name, payloadIndex.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity))
            )
          }
          (JsWitTypeNode.variantType(variantEntries), name.fold[js.UndefOr[String]](js.undefined)(identity))
        case DataType.PureEnumType(cases, name) =>
          (JsWitTypeNode.enumType(js.Array(cases: _*)), name.fold[js.UndefOr[String]](js.undefined)(identity))
        case DataType.ResultType(ok, err) =>
          val okIdx  = ok.map(buildNode).fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
          val errIdx = err.map(buildNode).fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
          (JsWitTypeNode.resultType(okIdx, errIdx), js.undefined: js.UndefOr[String])
      }

      nodes(index) = JsNamedWitTypeNode(typeVariant, name = typeName)
      index
    }

    private def newNode(): Int = {
      val placeholder = JsNamedWitTypeNode(JsShape.tagOnly[JsWitTypeNode]("__placeholder"))
      nodes.push(placeholder)
      nodes.length - 1
    }
  }
}
