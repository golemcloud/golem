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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName
import scala.scalajs.js.typedarray.Uint8Array

// ---------------------------------------------------------------------------
// golem:core/types@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- Simple record types ---

@js.native
sealed trait JsUuid extends js.Object {
  def highBits: js.BigInt = js.native
  def lowBits: js.BigInt  = js.native
}

object JsUuid {
  def apply(highBits: js.BigInt, lowBits: js.BigInt): JsUuid =
    js.Dynamic.literal("highBits" -> highBits, "lowBits" -> lowBits).asInstanceOf[JsUuid]
}

@js.native
sealed trait JsComponentId extends js.Object {
  def uuid: JsUuid = js.native
}

object JsComponentId {
  def apply(uuid: JsUuid): JsComponentId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsComponentId]
}

@js.native
sealed trait JsAgentId extends js.Object {
  def componentId: JsComponentId = js.native
  def agentId: String            = js.native
}

object JsAgentId {
  def apply(componentId: JsComponentId, agentId: String): JsAgentId =
    js.Dynamic.literal("componentId" -> componentId, "agentId" -> agentId).asInstanceOf[JsAgentId]
}

@js.native
sealed trait JsAccountId extends js.Object {
  def uuid: JsUuid = js.native
}

object JsAccountId {
  def apply(uuid: JsUuid): JsAccountId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsAccountId]
}

@js.native
sealed trait JsPromiseId extends js.Object {
  def agentId: JsAgentId  = js.native
  def oplogIdx: js.BigInt = js.native
}

object JsPromiseId {
  def apply(agentId: JsAgentId, oplogIdx: js.BigInt): JsPromiseId =
    js.Dynamic.literal("agentId" -> agentId, "oplogIdx" -> oplogIdx).asInstanceOf[JsPromiseId]
}

@js.native
sealed trait JsUri extends js.Object {
  def value: String = js.native
}

object JsUri {
  def apply(value: String): JsUri =
    js.Dynamic.literal("value" -> value).asInstanceOf[JsUri]
}

// ---------------------------------------------------------------------------
// WitTypeNode  –  tagged union (22 cases)
// ---------------------------------------------------------------------------

@js.native
sealed trait JsWitTypeNode extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsWitTypeNodeRecordType extends JsWitTypeNode {
  @JSName("val") def value: js.Array[js.Tuple2[String, JsNodeIndex]] = js.native
}

@js.native
sealed trait JsWitTypeNodeVariantType extends JsWitTypeNode {
  @JSName("val") def value: js.Array[js.Tuple2[String, js.UndefOr[JsNodeIndex]]] = js.native
}

@js.native
sealed trait JsWitTypeNodeEnumType extends JsWitTypeNode {
  @JSName("val") def value: js.Array[String] = js.native
}

@js.native
sealed trait JsWitTypeNodeFlagsType extends JsWitTypeNode {
  @JSName("val") def value: js.Array[String] = js.native
}

@js.native
sealed trait JsWitTypeNodeTupleType extends JsWitTypeNode {
  @JSName("val") def value: js.Array[JsNodeIndex] = js.native
}

@js.native
sealed trait JsWitTypeNodeListType extends JsWitTypeNode {
  @JSName("val") def value: JsNodeIndex = js.native
}

@js.native
sealed trait JsWitTypeNodeOptionType extends JsWitTypeNode {
  @JSName("val") def value: JsNodeIndex = js.native
}

@js.native
sealed trait JsWitTypeNodeResultType extends JsWitTypeNode {
  @JSName("val") def value: js.Tuple2[js.UndefOr[JsNodeIndex], js.UndefOr[JsNodeIndex]] = js.native
}

@js.native
sealed trait JsWitTypeNodeHandleType extends JsWitTypeNode {
  @JSName("val") def value: js.Tuple2[JsResourceId, JsResourceMode] = js.native
}

object JsWitTypeNode {
  def recordType(fields: js.Array[js.Tuple2[String, JsNodeIndex]]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("record-type", fields)

  def variantType(cases: js.Array[js.Tuple2[String, js.UndefOr[JsNodeIndex]]]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("variant-type", cases)

  def enumType(cases: js.Array[String]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("enum-type", cases)

  def flagsType(flags: js.Array[String]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("flags-type", flags)

  def tupleType(elements: js.Array[JsNodeIndex]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("tuple-type", elements)

  def listType(element: JsNodeIndex): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("list-type", element.asInstanceOf[js.Any])

  def optionType(element: JsNodeIndex): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("option-type", element.asInstanceOf[js.Any])

  def resultType(ok: js.UndefOr[JsNodeIndex], err: js.UndefOr[JsNodeIndex]): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("result-type", js.Tuple2(ok, err))

  def primU8Type: JsWitTypeNode     = JsShape.tagOnly[JsWitTypeNode]("prim-u8-type")
  def primU16Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-u16-type")
  def primU32Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-u32-type")
  def primU64Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-u64-type")
  def primS8Type: JsWitTypeNode     = JsShape.tagOnly[JsWitTypeNode]("prim-s8-type")
  def primS16Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-s16-type")
  def primS32Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-s32-type")
  def primS64Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-s64-type")
  def primF32Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-f32-type")
  def primF64Type: JsWitTypeNode    = JsShape.tagOnly[JsWitTypeNode]("prim-f64-type")
  def primCharType: JsWitTypeNode   = JsShape.tagOnly[JsWitTypeNode]("prim-char-type")
  def primBoolType: JsWitTypeNode   = JsShape.tagOnly[JsWitTypeNode]("prim-bool-type")
  def primStringType: JsWitTypeNode = JsShape.tagOnly[JsWitTypeNode]("prim-string-type")

  def handleType(resourceId: JsResourceId, mode: JsResourceMode): JsWitTypeNode =
    JsShape.tagged[JsWitTypeNode]("handle-type", js.Tuple2(resourceId, mode))
}

// ---------------------------------------------------------------------------
// NamedWitTypeNode, WitType
// ---------------------------------------------------------------------------

@js.native
sealed trait JsNamedWitTypeNode extends js.Object {
  def name: js.UndefOr[String]           = js.native
  def owner: js.UndefOr[String]          = js.native
  @JSName("type") def typ: JsWitTypeNode = js.native
}

object JsNamedWitTypeNode {
  def apply(
    typ: JsWitTypeNode,
    name: js.UndefOr[String] = js.undefined,
    owner: js.UndefOr[String] = js.undefined
  ): JsNamedWitTypeNode = {
    val obj = js.Dynamic.literal("type" -> typ.asInstanceOf[js.Any])
    name.foreach(n => obj.updateDynamic("name")(n))
    owner.foreach(o => obj.updateDynamic("owner")(o))
    obj.asInstanceOf[JsNamedWitTypeNode]
  }
}

@js.native
sealed trait JsWitType extends js.Object {
  def nodes: js.Array[JsNamedWitTypeNode] = js.native
}

object JsWitType {
  def apply(nodes: js.Array[JsNamedWitTypeNode]): JsWitType =
    js.Dynamic.literal("nodes" -> nodes).asInstanceOf[JsWitType]
}

// ---------------------------------------------------------------------------
// WitNode  –  tagged union (22 cases)
// ---------------------------------------------------------------------------

@js.native
sealed trait JsWitNode extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsWitNodeRecordValue extends JsWitNode {
  @JSName("val") def value: js.Array[JsNodeIndex] = js.native
}

@js.native
sealed trait JsWitNodeVariantValue extends JsWitNode {
  @JSName("val") def value: js.Tuple2[Int, js.UndefOr[JsNodeIndex]] = js.native
}

@js.native
sealed trait JsWitNodeTupleValue extends JsWitNode {
  @JSName("val") def value: js.Array[JsNodeIndex] = js.native
}

@js.native
sealed trait JsWitNodeListValue extends JsWitNode {
  @JSName("val") def value: js.Array[JsNodeIndex] = js.native
}

@js.native
sealed trait JsWitNodeOptionValue extends JsWitNode {
  @JSName("val") def value: js.UndefOr[JsNodeIndex] = js.native
}

@js.native
sealed trait JsWitNodeResultValue extends JsWitNode {
  @JSName("val") def value: JsResult[js.UndefOr[JsNodeIndex], js.UndefOr[JsNodeIndex]] = js.native
}

@js.native
sealed trait JsWitNodePrimS32 extends JsWitNode {
  @JSName("val") def value: Int = js.native
}

@js.native
sealed trait JsWitNodePrimS64 extends JsWitNode {
  @JSName("val") def value: js.BigInt = js.native
}

@js.native
sealed trait JsWitNodePrimBool extends JsWitNode {
  @JSName("val") def value: Boolean = js.native
}

@js.native
sealed trait JsWitNodePrimFloat64 extends JsWitNode {
  @JSName("val") def value: Double = js.native
}

@js.native
sealed trait JsWitNodePrimString extends JsWitNode {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsWitNodeEnumValue extends JsWitNode {
  @JSName("val") def value: Int = js.native
}

@js.native
sealed trait JsWitNodeFlagsValue extends JsWitNode {
  @JSName("val") def value: js.Array[Boolean] = js.native
}

@js.native
sealed trait JsWitNodePrimU8 extends JsWitNode {
  @JSName("val") def value: Short = js.native
}

@js.native
sealed trait JsWitNodePrimU16 extends JsWitNode {
  @JSName("val") def value: Int = js.native
}

@js.native
sealed trait JsWitNodePrimU32 extends JsWitNode {
  @JSName("val") def value: Double = js.native
}

@js.native
sealed trait JsWitNodePrimU64 extends JsWitNode {
  @JSName("val") def value: js.BigInt = js.native
}

@js.native
sealed trait JsWitNodePrimS8 extends JsWitNode {
  @JSName("val") def value: Byte = js.native
}

@js.native
sealed trait JsWitNodePrimS16 extends JsWitNode {
  @JSName("val") def value: Short = js.native
}

@js.native
sealed trait JsWitNodePrimFloat32 extends JsWitNode {
  @JSName("val") def value: Float = js.native
}

@js.native
sealed trait JsWitNodePrimChar extends JsWitNode {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsWitNodeHandle extends JsWitNode {
  @JSName("val") def value: js.Tuple2[JsUri, js.BigInt] = js.native
}

object JsWitNode {
  def recordValue(fields: js.Array[JsNodeIndex]): JsWitNode =
    JsShape.tagged[JsWitNode]("record-value", fields)

  def variantValue(caseIndex: Int, value: js.UndefOr[JsNodeIndex]): JsWitNode =
    JsShape.tagged[JsWitNode]("variant-value", js.Tuple2(caseIndex, value))

  def enumValue(caseIndex: Int): JsWitNode =
    JsShape.tagged[JsWitNode]("enum-value", caseIndex.asInstanceOf[js.Any])

  def flagsValue(flags: js.Array[Boolean]): JsWitNode =
    JsShape.tagged[JsWitNode]("flags-value", flags)

  def tupleValue(elements: js.Array[JsNodeIndex]): JsWitNode =
    JsShape.tagged[JsWitNode]("tuple-value", elements)

  def listValue(elements: js.Array[JsNodeIndex]): JsWitNode =
    JsShape.tagged[JsWitNode]("list-value", elements)

  def optionValue(value: js.UndefOr[JsNodeIndex]): JsWitNode =
    JsShape.taggedOptional[JsWitNode]("option-value", value.map(_.asInstanceOf[js.Any]))

  def resultValue(result: JsResult[js.UndefOr[JsNodeIndex], js.UndefOr[JsNodeIndex]]): JsWitNode =
    JsShape.tagged[JsWitNode]("result-value", result)

  def primU8(value: Short): JsWitNode       = JsShape.tagged[JsWitNode]("prim-u8", value.asInstanceOf[js.Any])
  def primU16(value: Int): JsWitNode        = JsShape.tagged[JsWitNode]("prim-u16", value.asInstanceOf[js.Any])
  def primU32(value: Double): JsWitNode     = JsShape.tagged[JsWitNode]("prim-u32", value.asInstanceOf[js.Any])
  def primU64(value: js.BigInt): JsWitNode  = JsShape.tagged[JsWitNode]("prim-u64", value)
  def primS8(value: Byte): JsWitNode        = JsShape.tagged[JsWitNode]("prim-s8", value.asInstanceOf[js.Any])
  def primS16(value: Short): JsWitNode      = JsShape.tagged[JsWitNode]("prim-s16", value.asInstanceOf[js.Any])
  def primS32(value: Int): JsWitNode        = JsShape.tagged[JsWitNode]("prim-s32", value.asInstanceOf[js.Any])
  def primS64(value: js.BigInt): JsWitNode  = JsShape.tagged[JsWitNode]("prim-s64", value)
  def primFloat32(value: Float): JsWitNode  = JsShape.tagged[JsWitNode]("prim-float32", value.asInstanceOf[js.Any])
  def primFloat64(value: Double): JsWitNode = JsShape.tagged[JsWitNode]("prim-float64", value.asInstanceOf[js.Any])
  def primChar(value: String): JsWitNode    = JsShape.tagged[JsWitNode]("prim-char", value.asInstanceOf[js.Any])
  def primBool(value: Boolean): JsWitNode   = JsShape.tagged[JsWitNode]("prim-bool", value.asInstanceOf[js.Any])
  def primString(value: String): JsWitNode  = JsShape.tagged[JsWitNode]("prim-string", value.asInstanceOf[js.Any])

  def handle(uri: JsUri, resourceId: js.BigInt): JsWitNode =
    JsShape.tagged[JsWitNode]("handle", js.Tuple2(uri, resourceId))
}

// ---------------------------------------------------------------------------
// WitValue, ValueAndType
// ---------------------------------------------------------------------------

@js.native
sealed trait JsWitValue extends js.Object {
  def nodes: js.Array[JsWitNode] = js.native
}

object JsWitValue {
  def apply(nodes: js.Array[JsWitNode]): JsWitValue =
    js.Dynamic.literal("nodes" -> nodes).asInstanceOf[JsWitValue]
}

@js.native
sealed trait JsValueAndType extends js.Object {
  def value: JsWitValue = js.native
  def typ: JsWitType    = js.native
}

object JsValueAndType {
  def apply(value: JsWitValue, typ: JsWitType): JsValueAndType =
    js.Dynamic.literal("value" -> value, "typ" -> typ).asInstanceOf[JsValueAndType]
}

// ---------------------------------------------------------------------------
// Result<T, E>
// ---------------------------------------------------------------------------

@js.native
sealed trait JsResult[+T, +E] extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsOk[+T] extends JsResult[T, Nothing] {
  @JSName("val") def value: T = js.native
}

@js.native
sealed trait JsErr[+E] extends JsResult[Nothing, E] {
  @JSName("val") def value: E = js.native
}

object JsResult {
  def ok[T](value: T): JsResult[T, Nothing] =
    JsShape.tagged[JsResult[T, Nothing]]("ok", value.asInstanceOf[js.Any])

  def err[E](value: E): JsResult[Nothing, E] =
    JsShape.tagged[JsResult[Nothing, E]]("err", value.asInstanceOf[js.Any])

  def okOptional[T](value: js.UndefOr[T]): JsResult[js.UndefOr[T], Nothing] =
    JsShape.taggedOptional[JsResult[js.UndefOr[T], Nothing]]("ok", value.map(_.asInstanceOf[js.Any]))

  def errOptional[E](value: js.UndefOr[E]): JsResult[Nothing, js.UndefOr[E]] =
    JsShape.taggedOptional[JsResult[Nothing, js.UndefOr[E]]]("err", value.map(_.asInstanceOf[js.Any]))
}

// ---------------------------------------------------------------------------
// Text / Binary types
// ---------------------------------------------------------------------------

@js.native
sealed trait JsTextType extends js.Object {
  def languageCode: String = js.native
}

object JsTextType {
  def apply(languageCode: String): JsTextType =
    js.Dynamic.literal("languageCode" -> languageCode).asInstanceOf[JsTextType]
}

@js.native
sealed trait JsTextSource extends js.Object {
  def data: String                     = js.native
  def textType: js.UndefOr[JsTextType] = js.native
}

object JsTextSource {
  def apply(data: String, textType: js.UndefOr[JsTextType] = js.undefined): JsTextSource = {
    val obj = js.Dynamic.literal("data" -> data)
    textType.foreach(tt => obj.updateDynamic("textType")(tt))
    obj.asInstanceOf[JsTextSource]
  }
}

@js.native
sealed trait JsTextReference extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsTextReferenceUrl extends JsTextReference {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsTextReferenceInline extends JsTextReference {
  @JSName("val") def value: JsTextSource = js.native
}

object JsTextReference {
  def url(value: String): JsTextReference =
    JsShape.tagged[JsTextReference]("url", value.asInstanceOf[js.Any])

  def inline(value: JsTextSource): JsTextReference =
    JsShape.tagged[JsTextReference]("inline", value)
}

@js.native
sealed trait JsBinaryType extends js.Object {
  def mimeType: String = js.native
}

object JsBinaryType {
  def apply(mimeType: String): JsBinaryType =
    js.Dynamic.literal("mimeType" -> mimeType).asInstanceOf[JsBinaryType]
}

@js.native
sealed trait JsBinarySource extends js.Object {
  def data: Uint8Array         = js.native
  def binaryType: JsBinaryType = js.native
}

object JsBinarySource {
  def apply(data: Uint8Array, binaryType: JsBinaryType): JsBinarySource =
    js.Dynamic.literal("data" -> data, "binaryType" -> binaryType).asInstanceOf[JsBinarySource]
}

@js.native
sealed trait JsBinaryReference extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsBinaryReferenceUrl extends JsBinaryReference {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsBinaryReferenceInline extends JsBinaryReference {
  @JSName("val") def value: JsBinarySource = js.native
}

object JsBinaryReference {
  def url(value: String): JsBinaryReference =
    JsShape.tagged[JsBinaryReference]("url", value.asInstanceOf[js.Any])

  def inline(value: JsBinarySource): JsBinaryReference =
    JsShape.tagged[JsBinaryReference]("inline", value)
}

@js.native
sealed trait JsTextDescriptor extends js.Object {
  def restrictions: js.UndefOr[js.Array[JsTextType]] = js.native
}

object JsTextDescriptor {
  def apply(restrictions: js.UndefOr[js.Array[JsTextType]] = js.undefined): JsTextDescriptor = {
    val obj = js.Dynamic.literal()
    restrictions.foreach(r => obj.updateDynamic("restrictions")(r))
    obj.asInstanceOf[JsTextDescriptor]
  }
}

@js.native
sealed trait JsBinaryDescriptor extends js.Object {
  def restrictions: js.UndefOr[js.Array[JsBinaryType]] = js.native
}

object JsBinaryDescriptor {
  def apply(restrictions: js.UndefOr[js.Array[JsBinaryType]] = js.undefined): JsBinaryDescriptor = {
    val obj = js.Dynamic.literal()
    restrictions.foreach(r => obj.updateDynamic("restrictions")(r))
    obj.asInstanceOf[JsBinaryDescriptor]
  }
}

// ---------------------------------------------------------------------------
// ElementSchema, ElementValue
// ---------------------------------------------------------------------------

@js.native
sealed trait JsElementSchema extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsElementSchemaComponentModel extends JsElementSchema {
  @JSName("val") def value: JsWitType = js.native
}

@js.native
sealed trait JsElementSchemaUnstructuredText extends JsElementSchema {
  @JSName("val") def value: JsTextDescriptor = js.native
}

@js.native
sealed trait JsElementSchemaUnstructuredBinary extends JsElementSchema {
  @JSName("val") def value: JsBinaryDescriptor = js.native
}

object JsElementSchema {
  def componentModel(witType: JsWitType): JsElementSchema =
    JsShape.tagged[JsElementSchema]("component-model", witType)

  def unstructuredText(descriptor: JsTextDescriptor): JsElementSchema =
    JsShape.tagged[JsElementSchema]("unstructured-text", descriptor)

  def unstructuredBinary(descriptor: JsBinaryDescriptor): JsElementSchema =
    JsShape.tagged[JsElementSchema]("unstructured-binary", descriptor)
}

@js.native
sealed trait JsElementValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsElementValueComponentModel extends JsElementValue {
  @JSName("val") def value: JsWitValue = js.native
}

@js.native
sealed trait JsElementValueUnstructuredText extends JsElementValue {
  @JSName("val") def value: JsTextReference = js.native
}

@js.native
sealed trait JsElementValueUnstructuredBinary extends JsElementValue {
  @JSName("val") def value: JsBinaryReference = js.native
}

object JsElementValue {
  def componentModel(witValue: JsWitValue): JsElementValue =
    JsShape.tagged[JsElementValue]("component-model", witValue)

  def unstructuredText(textRef: JsTextReference): JsElementValue =
    JsShape.tagged[JsElementValue]("unstructured-text", textRef)

  def unstructuredBinary(binaryRef: JsBinaryReference): JsElementValue =
    JsShape.tagged[JsElementValue]("unstructured-binary", binaryRef)
}

// ---------------------------------------------------------------------------
// DataSchema, DataValue
// ---------------------------------------------------------------------------

@js.native
sealed trait JsDataSchema extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsDataSchemaTuple extends JsDataSchema {
  @JSName("val") def value: js.Array[js.Tuple2[String, JsElementSchema]] = js.native
}

@js.native
sealed trait JsDataSchemaMultimodal extends JsDataSchema {
  @JSName("val") def value: js.Array[js.Tuple2[String, JsElementSchema]] = js.native
}

object JsDataSchema {
  def tuple(elements: js.Array[js.Tuple2[String, JsElementSchema]]): JsDataSchema =
    JsShape.tagged[JsDataSchema]("tuple", elements)

  def multimodal(elements: js.Array[js.Tuple2[String, JsElementSchema]]): JsDataSchema =
    JsShape.tagged[JsDataSchema]("multimodal", elements)
}

@js.native
sealed trait JsDataValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsDataValueTuple extends JsDataValue {
  @JSName("val") def value: js.Array[JsElementValue] = js.native
}

@js.native
sealed trait JsDataValueMultimodal extends JsDataValue {
  @JSName("val") def value: js.Array[js.Tuple2[String, JsElementValue]] = js.native
}

object JsDataValue {
  def tuple(elements: js.Array[JsElementValue]): JsDataValue =
    JsShape.tagged[JsDataValue]("tuple", elements)

  def multimodal(entries: js.Array[js.Tuple2[String, JsElementValue]]): JsDataValue =
    JsShape.tagged[JsDataValue]("multimodal", entries)
}
