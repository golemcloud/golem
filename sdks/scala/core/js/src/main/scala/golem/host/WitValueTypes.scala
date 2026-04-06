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

package golem.host

import golem.host.js._

import scala.scalajs.js

/**
 * Scala types for `golem:core/types@1.5.0` value serialization types.
 *
 * These model the WIT `wit-value`, `wit-node`, `wit-type`, `wit-type-node`, and
 * `value-and-type` types used by the durability and oplog APIs.
 */
object WitValueTypes {

  type NodeIndex = Int

  // --- WIT: wit-node variant (22 cases) ---

  sealed trait WitNode extends Product with Serializable
  object WitNode {
    final case class RecordValue(fields: List[NodeIndex])                       extends WitNode
    final case class VariantValue(caseIndex: Int, value: Option[NodeIndex])     extends WitNode
    final case class EnumValue(caseIndex: Int)                                  extends WitNode
    final case class FlagsValue(flags: List[Boolean])                           extends WitNode
    final case class TupleValue(elements: List[NodeIndex])                      extends WitNode
    final case class ListValue(elements: List[NodeIndex])                       extends WitNode
    final case class OptionValue(value: Option[NodeIndex])                      extends WitNode
    final case class ResultValue(ok: Option[NodeIndex], err: Option[NodeIndex]) extends WitNode
    final case class PrimU8(value: Short)                                       extends WitNode
    final case class PrimU16(value: Int)                                        extends WitNode
    final case class PrimU32(value: Long)                                       extends WitNode
    final case class PrimU64(value: BigInt)                                     extends WitNode
    final case class PrimS8(value: Byte)                                        extends WitNode
    final case class PrimS16(value: Short)                                      extends WitNode
    final case class PrimS32(value: Int)                                        extends WitNode
    final case class PrimS64(value: Long)                                       extends WitNode
    final case class PrimFloat32(value: Float)                                  extends WitNode
    final case class PrimFloat64(value: Double)                                 extends WitNode
    final case class PrimChar(value: Char)                                      extends WitNode
    final case class PrimBool(value: Boolean)                                   extends WitNode
    final case class PrimString(value: String)                                  extends WitNode
    final case class Handle(uri: String, resourceId: BigInt)                    extends WitNode

    def fromJs(raw: JsWitNode): WitNode =
      raw.tag match {
        case "record-value" =>
          val arr = raw.asInstanceOf[JsWitNodeRecordValue].value
          RecordValue(arr.toList)
        case "variant-value" =>
          val tup      = raw.asInstanceOf[JsWitNodeVariantValue].value
          val caseIdx  = tup._1
          val valueOpt = tup._2.toOption
          VariantValue(caseIdx, valueOpt)
        case "enum-value" =>
          EnumValue(raw.asInstanceOf[JsWitNodeEnumValue].value)
        case "flags-value" =>
          FlagsValue(raw.asInstanceOf[JsWitNodeFlagsValue].value.toList)
        case "tuple-value" =>
          val arr = raw.asInstanceOf[JsWitNodeTupleValue].value
          TupleValue(arr.toList)
        case "list-value" =>
          val arr = raw.asInstanceOf[JsWitNodeListValue].value
          ListValue(arr.toList)
        case "option-value" =>
          val v = raw.asInstanceOf[JsWitNodeOptionValue].value
          OptionValue(v.toOption)
        case "result-value" =>
          val result = raw.asInstanceOf[JsWitNodeResultValue].value
          if (result.tag == "ok") {
            val okVal = result.asInstanceOf[JsOk[js.UndefOr[JsNodeIndex]]].value
            ResultValue(ok = okVal.toOption, err = None)
          } else {
            val errVal = result.asInstanceOf[JsErr[js.UndefOr[JsNodeIndex]]].value
            ResultValue(ok = None, err = errVal.toOption)
          }
        case "prim-u8" =>
          PrimU8(raw.asInstanceOf[JsWitNodePrimU8].value)
        case "prim-u16" =>
          PrimU16(raw.asInstanceOf[JsWitNodePrimU16].value)
        case "prim-u32" =>
          PrimU32(raw.asInstanceOf[JsWitNodePrimU32].value.toLong)
        case "prim-u64" =>
          PrimU64(BigInt(raw.asInstanceOf[JsWitNodePrimU64].value.toString))
        case "prim-s8" =>
          PrimS8(raw.asInstanceOf[JsWitNodePrimS8].value)
        case "prim-s16" =>
          PrimS16(raw.asInstanceOf[JsWitNodePrimS16].value)
        case "prim-s32" =>
          PrimS32(raw.asInstanceOf[JsWitNodePrimS32].value)
        case "prim-s64" =>
          PrimS64(BigInt(raw.asInstanceOf[JsWitNodePrimS64].value.toString).toLong)
        case "prim-float32" =>
          PrimFloat32(raw.asInstanceOf[JsWitNodePrimFloat32].value)
        case "prim-float64" =>
          PrimFloat64(raw.asInstanceOf[JsWitNodePrimFloat64].value)
        case "prim-char" =>
          PrimChar(raw.asInstanceOf[JsWitNodePrimChar].value.charAt(0))
        case "prim-bool" =>
          PrimBool(raw.asInstanceOf[JsWitNodePrimBool].value)
        case "prim-string" =>
          PrimString(raw.asInstanceOf[JsWitNodePrimString].value)
        case "handle" =>
          val tup = raw.asInstanceOf[JsWitNodeHandle].value
          Handle(tup._1.value, BigInt(tup._2.toString))
        case other => throw new IllegalArgumentException(s"Unknown WitNode tag: $other")
      }

    def toJs(node: WitNode): JsWitNode = node match {
      case RecordValue(fields) =>
        JsWitNode.recordValue(js.Array(fields: _*))
      case VariantValue(ci, v) =>
        val optIdx: js.UndefOr[JsNodeIndex] = v.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
        JsWitNode.variantValue(ci, optIdx)
      case EnumValue(ci)     => JsWitNode.enumValue(ci)
      case FlagsValue(flags) => JsWitNode.flagsValue(js.Array(flags: _*))
      case TupleValue(elems) => JsWitNode.tupleValue(js.Array(elems: _*))
      case ListValue(elems)  => JsWitNode.listValue(js.Array(elems: _*))
      case OptionValue(v)    =>
        val optIdx: js.UndefOr[JsNodeIndex] = v.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
        JsWitNode.optionValue(optIdx)
      case ResultValue(ok, err) =>
        val inner = if (ok.isDefined || err.isEmpty) {
          val okVal: js.UndefOr[JsNodeIndex] = ok.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
          JsResult.okOptional(okVal)
        } else {
          val errVal: js.UndefOr[JsNodeIndex] = err.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
          JsResult.errOptional(errVal)
        }
        JsWitNode.resultValue(inner)
      case PrimU8(v)        => JsWitNode.primU8(v)
      case PrimU16(v)       => JsWitNode.primU16(v)
      case PrimU32(v)       => JsWitNode.primU32(v.toDouble)
      case PrimU64(v)       => JsWitNode.primU64(js.BigInt(v.toString))
      case PrimS8(v)        => JsWitNode.primS8(v)
      case PrimS16(v)       => JsWitNode.primS16(v)
      case PrimS32(v)       => JsWitNode.primS32(v)
      case PrimS64(v)       => JsWitNode.primS64(js.BigInt(v.toString))
      case PrimFloat32(v)   => JsWitNode.primFloat32(v)
      case PrimFloat64(v)   => JsWitNode.primFloat64(v)
      case PrimChar(v)      => JsWitNode.primChar(v.toString)
      case PrimBool(v)      => JsWitNode.primBool(v)
      case PrimString(v)    => JsWitNode.primString(v)
      case Handle(uri, rid) => JsWitNode.handle(JsUri(uri), js.BigInt(rid.toString))
    }
  }

  // --- WIT: wit-value record ---

  final case class WitValue(nodes: List[WitNode])

  object WitValue {
    def fromJs(raw: JsWitValue): WitValue =
      WitValue(raw.nodes.toList.map(WitNode.fromJs))

    def toJs(wv: WitValue): JsWitValue = {
      val arr = js.Array[JsWitNode]()
      wv.nodes.foreach(n => arr.push(WitNode.toJs(n)))
      JsWitValue(arr)
    }

  }

  // --- WIT: resource-mode enum ---

  sealed trait ResourceMode extends Product with Serializable
  object ResourceMode {
    case object Owned    extends ResourceMode
    case object Borrowed extends ResourceMode

    def fromString(s: String): ResourceMode = s match {
      case "owned"    => Owned
      case "borrowed" => Borrowed
      case _          => Owned
    }
  }

  // --- WIT: wit-type-node variant (21 cases) ---

  sealed trait WitTypeNode extends Product with Serializable
  object WitTypeNode {
    final case class RecordType(fields: List[(String, NodeIndex)])             extends WitTypeNode
    final case class VariantType(cases: List[(String, Option[NodeIndex])])     extends WitTypeNode
    final case class EnumType(cases: List[String])                             extends WitTypeNode
    final case class FlagsType(flags: List[String])                            extends WitTypeNode
    final case class TupleType(elements: List[NodeIndex])                      extends WitTypeNode
    final case class ListType(element: NodeIndex)                              extends WitTypeNode
    final case class OptionType(element: NodeIndex)                            extends WitTypeNode
    final case class ResultType(ok: Option[NodeIndex], err: Option[NodeIndex]) extends WitTypeNode
    case object PrimU8Type                                                     extends WitTypeNode
    case object PrimU16Type                                                    extends WitTypeNode
    case object PrimU32Type                                                    extends WitTypeNode
    case object PrimU64Type                                                    extends WitTypeNode
    case object PrimS8Type                                                     extends WitTypeNode
    case object PrimS16Type                                                    extends WitTypeNode
    case object PrimS32Type                                                    extends WitTypeNode
    case object PrimS64Type                                                    extends WitTypeNode
    case object PrimF32Type                                                    extends WitTypeNode
    case object PrimF64Type                                                    extends WitTypeNode
    case object PrimCharType                                                   extends WitTypeNode
    case object PrimBoolType                                                   extends WitTypeNode
    case object PrimStringType                                                 extends WitTypeNode
    final case class HandleType(resourceId: BigInt, mode: ResourceMode)        extends WitTypeNode

    def fromJs(raw: JsWitTypeNode): WitTypeNode =
      raw.tag match {
        case "record-type" =>
          val arr = raw.asInstanceOf[JsWitTypeNodeRecordType].value
          RecordType(arr.toList.map(t => (t._1, t._2)))
        case "variant-type" =>
          val arr = raw.asInstanceOf[JsWitTypeNodeVariantType].value
          VariantType(arr.toList.map { t =>
            (t._1, t._2.toOption)
          })
        case "enum-type" =>
          val arr = raw.asInstanceOf[JsWitTypeNodeEnumType].value
          EnumType(arr.toList)
        case "flags-type" =>
          val arr = raw.asInstanceOf[JsWitTypeNodeFlagsType].value
          FlagsType(arr.toList)
        case "tuple-type" =>
          val arr = raw.asInstanceOf[JsWitTypeNodeTupleType].value
          TupleType(arr.toList)
        case "list-type" =>
          ListType(raw.asInstanceOf[JsWitTypeNodeListType].value)
        case "option-type" =>
          OptionType(raw.asInstanceOf[JsWitTypeNodeOptionType].value)
        case "result-type" =>
          val tup = raw.asInstanceOf[JsWitTypeNodeResultType].value
          ResultType(tup._1.toOption, tup._2.toOption)
        case "prim-u8-type"     => PrimU8Type
        case "prim-u16-type"    => PrimU16Type
        case "prim-u32-type"    => PrimU32Type
        case "prim-u64-type"    => PrimU64Type
        case "prim-s8-type"     => PrimS8Type
        case "prim-s16-type"    => PrimS16Type
        case "prim-s32-type"    => PrimS32Type
        case "prim-s64-type"    => PrimS64Type
        case "prim-f32-type"    => PrimF32Type
        case "prim-f64-type"    => PrimF64Type
        case "prim-char-type"   => PrimCharType
        case "prim-bool-type"   => PrimBoolType
        case "prim-string-type" => PrimStringType
        case "handle-type"      =>
          val tup  = raw.asInstanceOf[JsWitTypeNodeHandleType].value
          val rid  = BigInt(tup._1.toString)
          val mode = ResourceMode.fromString(tup._2)
          HandleType(rid, mode)
        case other => throw new IllegalArgumentException(s"Unknown WitTypeNode tag: $other")
      }

    def toJs(node: WitTypeNode): JsWitTypeNode = node match {
      case RecordType(fields) =>
        val arr = js.Array[js.Tuple2[String, JsNodeIndex]]()
        fields.foreach { case (name, idx) => arr.push(js.Tuple2(name, idx)) }
        JsWitTypeNode.recordType(arr)
      case VariantType(cases) =>
        val arr = js.Array[js.Tuple2[String, js.UndefOr[JsNodeIndex]]]()
        cases.foreach { case (name, opt) =>
          val v: js.UndefOr[JsNodeIndex] = opt.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
          arr.push(js.Tuple2(name, v))
        }
        JsWitTypeNode.variantType(arr)
      case EnumType(cases)     => JsWitTypeNode.enumType(js.Array(cases: _*))
      case FlagsType(flags)    => JsWitTypeNode.flagsType(js.Array(flags: _*))
      case TupleType(elems)    => JsWitTypeNode.tupleType(js.Array(elems: _*))
      case ListType(elem)      => JsWitTypeNode.listType(elem)
      case OptionType(elem)    => JsWitTypeNode.optionType(elem)
      case ResultType(ok, err) =>
        val okVal: js.UndefOr[JsNodeIndex]  = ok.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
        val errVal: js.UndefOr[JsNodeIndex] = err.fold[js.UndefOr[JsNodeIndex]](js.undefined)(identity)
        JsWitTypeNode.resultType(okVal, errVal)
      case PrimU8Type            => JsWitTypeNode.primU8Type
      case PrimU16Type           => JsWitTypeNode.primU16Type
      case PrimU32Type           => JsWitTypeNode.primU32Type
      case PrimU64Type           => JsWitTypeNode.primU64Type
      case PrimS8Type            => JsWitTypeNode.primS8Type
      case PrimS16Type           => JsWitTypeNode.primS16Type
      case PrimS32Type           => JsWitTypeNode.primS32Type
      case PrimS64Type           => JsWitTypeNode.primS64Type
      case PrimF32Type           => JsWitTypeNode.primF32Type
      case PrimF64Type           => JsWitTypeNode.primF64Type
      case PrimCharType          => JsWitTypeNode.primCharType
      case PrimBoolType          => JsWitTypeNode.primBoolType
      case PrimStringType        => JsWitTypeNode.primStringType
      case HandleType(rid, mode) =>
        val modeStr: String = mode match {
          case ResourceMode.Owned    => "owned"
          case ResourceMode.Borrowed => "borrowed"
        }
        JsWitTypeNode.handleType(js.BigInt(rid.toString), modeStr)
    }

  }

  // --- WIT: named-wit-type-node record ---

  final case class NamedWitTypeNode(
    name: Option[String],
    owner: Option[String],
    typeNode: WitTypeNode
  )

  object NamedWitTypeNode {
    def fromJs(raw: JsNamedWitTypeNode): NamedWitTypeNode = {
      val name  = raw.name.toOption
      val owner = raw.owner.toOption
      val tn    = WitTypeNode.fromJs(raw.typ)
      NamedWitTypeNode(name, owner, tn)
    }

    def toJs(n: NamedWitTypeNode): JsNamedWitTypeNode = {
      val nameVal: js.UndefOr[String]  = n.name.fold[js.UndefOr[String]](js.undefined)(identity)
      val ownerVal: js.UndefOr[String] = n.owner.fold[js.UndefOr[String]](js.undefined)(identity)
      JsNamedWitTypeNode(WitTypeNode.toJs(n.typeNode), nameVal, ownerVal)
    }

  }

  // --- WIT: wit-type record ---

  final case class WitType(nodes: List[NamedWitTypeNode])

  object WitType {
    def fromJs(raw: JsWitType): WitType =
      WitType(raw.nodes.toList.map(NamedWitTypeNode.fromJs))

    def toJs(wt: WitType): JsWitType = {
      val arr = js.Array[JsNamedWitTypeNode]()
      wt.nodes.foreach(n => arr.push(NamedWitTypeNode.toJs(n)))
      JsWitType(arr)
    }

  }

  // --- WIT: value-and-type record ---

  final case class ValueAndType(value: WitValue, typ: WitType)

  object ValueAndType {
    def fromJs(raw: JsValueAndType): ValueAndType = {
      val v = WitValue.fromJs(raw.value)
      val t = WitType.fromJs(raw.typ)
      ValueAndType(v, t)
    }

    def toJs(vat: ValueAndType): JsValueAndType = {
      val v = WitValue.toJs(vat.value)
      val t = WitType.toJs(vat.typ)
      JsValueAndType(v, t)
    }

  }
}
