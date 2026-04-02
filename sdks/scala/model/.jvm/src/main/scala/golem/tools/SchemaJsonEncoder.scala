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

import golem.data._
import ujson._

object SchemaJsonEncoder {
  def encode(schema: StructuredSchema): Value =
    schema match {
      case StructuredSchema.Tuple(elements) =>
        Obj(
          "tag" -> Str("tuple"),
          "val" -> Arr(elements.map(encodeElement): _*)
        )
      case StructuredSchema.Multimodal(elements) =>
        Obj(
          "tag" -> Str("multimodal"),
          "val" -> Arr(elements.map(encodeElement): _*)
        )
    }

  private def encodeElement(element: NamedElementSchema): Value =
    Arr(Str(element.name), encodeElementSchema(element.schema))

  private def encodeElementSchema(schema: ElementSchema): Value =
    schema match {
      case ElementSchema.Component(dataType) =>
        Obj(
          "tag" -> Str("component-model"),
          "val" -> WitTypeBuilderJson.build(dataType)
        )
      case ElementSchema.UnstructuredText(restrictions) =>
        Obj(
          "tag" -> Str("unstructured-text"),
          "val" -> encodeTextRestrictions(restrictions)
        )
      case ElementSchema.UnstructuredBinary(restrictions) =>
        Obj(
          "tag" -> Str("unstructured-binary"),
          "val" -> encodeBinaryRestrictions(restrictions)
        )
    }

  private def encodeTextRestrictions(restrictions: Option[List[String]]): Value =
    restrictions match {
      case Some(values) =>
        val arr = Arr(values.map(code => Obj("language-code" -> Str(code))): _*)
        Obj("tag" -> Str("some"), "val" -> arr)
      case None =>
        Obj("tag" -> Str("none"))
    }

  private def encodeBinaryRestrictions(restrictions: Option[List[String]]): Value =
    restrictions match {
      case Some(values) =>
        val arr = Arr(values.map(mime => Obj("mime-type" -> Str(mime))): _*)
        Obj("tag" -> Str("some"), "val" -> arr)
      case None =>
        Obj("tag" -> Str("none"))
    }

  private object WitTypeBuilderJson {

    def build(dataType: DataType): Value = {
      val builder = new Builder
      builder.buildNode(dataType)
      builder.result()
    }

    private final class Builder {
      private val nodes = collection.mutable.ArrayBuffer.empty[Value]

      def result(): Value =
        Obj("nodes" -> Arr(nodes.toSeq: _*))

      def buildNode(dataType: DataType): Int = {
        val index                                   = newNode()
        val (node: Value, typeName: Option[String]) = dataType match {
          case DataType.UnitType =>
            (tupleType(Seq.empty), None)
          case DataType.StringType =>
            (tagOnly("prim-string-type"), None)
          case DataType.BoolType =>
            (tagOnly("prim-bool-type"), None)
          case DataType.CharType =>
            (tagOnly("prim-char-type"), None)
          case DataType.ByteType =>
            (tagOnly("prim-s8-type"), None)
          case DataType.ShortType =>
            (tagOnly("prim-s16-type"), None)
          case DataType.IntType =>
            (tagOnly("prim-s32-type"), None)
          case DataType.LongType =>
            (tagOnly("prim-s64-type"), None)
          case DataType.FloatType =>
            (tagOnly("prim-f32-type"), None)
          case DataType.DoubleType =>
            (tagOnly("prim-f64-type"), None)
          case DataType.UByteType =>
            (tagOnly("prim-u8-type"), None)
          case DataType.UShortType =>
            (tagOnly("prim-u16-type"), None)
          case DataType.UIntType =>
            (tagOnly("prim-u32-type"), None)
          case DataType.ULongType =>
            (tagOnly("prim-u64-type"), None)
          case DataType.BigDecimalType =>
            (tagOnly("prim-string-type"), None)
          case DataType.UUIDType =>
            (tagOnly("prim-string-type"), None)
          case DataType.BytesType =>
            (listType(buildNode(DataType.IntType)), None)
          case DataType.Optional(of) =>
            (optionType(buildNode(of)), None)
          case DataType.ListType(of) =>
            (listType(buildNode(of)), None)
          case DataType.SetType(of) =>
            (listType(buildNode(of)), None)
          case DataType.MapType(keyType, valueType) =>
            val entryStruct = DataType.StructType(
              List(
                DataType.Field("key", keyType, optional = false),
                DataType.Field("value", valueType, optional = false)
              )
            )
            val entryIndex = buildNode(entryStruct)
            (listType(entryIndex), None)
          case DataType.TupleType(elements) =>
            (tupleType(elements.map(buildNode)), None)
          case DataType.StructType(fields, name) =>
            val fieldEntries = fields.map { field =>
              val idx = buildNode(field.dataType)
              Arr(Str(field.name), Num(idx))
            }
            (recordType(fieldEntries), name)
          case DataType.EnumType(cases, name) =>
            val variantEntries = cases.map { enumCase =>
              val payloadIndex        = enumCase.payload.map(buildNode)
              val payloadValue: Value = payloadIndex match {
                case Some(value) => Num(value)
                case None        => Null
              }
              Arr(Str(enumCase.name), payloadValue)
            }
            (variantType(variantEntries), name)
          case DataType.PureEnumType(cases, name) =>
            val variantEntries = cases.map { caseName =>
              Arr(Str(caseName), Null)
            }
            (variantType(variantEntries), name)
          case DataType.ResultType(ok, err) =>
            val okIndex  = ok.map(buildNode)
            val errIndex = err.map(buildNode)
            (
              Obj(
                "tag"   -> Str("result-type"),
                "ok"    -> okIndex.fold[Value](Null)(i => Num(i)),
                "error" -> errIndex.fold[Value](Null)(i => Num(i))
              ),
              None
            )
        }

        nodes(index) = namedNode(node, typeName)
        index
      }

      private def namedNode(typeNode: Value, name: Option[String]): Value = {
        val fields = collection.mutable.LinkedHashMap[String, Value]("type" -> typeNode)
        name.foreach(n => fields("name") = Str(n))
        Obj.from(fields)
      }

      private def tagOnly(tag: String): Value =
        Obj("tag" -> Str(tag))

      private def newNode(): Int = {
        nodes += Obj("type" -> Obj())
        nodes.length - 1
      }

      private def tupleType(values: Seq[Int]): Value =
        Obj("tag" -> Str("tuple-type"), "val" -> Arr(values.map(Num(_)): _*))

      private def listType(of: Int): Value =
        Obj("tag" -> Str("list-type"), "val" -> Num(of))

      private def optionType(of: Int): Value =
        Obj("tag" -> Str("option-type"), "val" -> Num(of))

      private def recordType(fields: Seq[Value]): Value =
        Obj("tag" -> Str("record-type"), "val" -> Arr(fields: _*))

      private def variantType(entries: Seq[Value]): Value =
        Obj("tag" -> Str("variant-type"), "val" -> Arr(entries: _*))
    }
  }
}
