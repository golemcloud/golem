/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host.js.schema

import golem.host.js.JsShape

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName
import scala.scalajs.js.typedarray.Uint8Array

// ---------------------------------------------------------------------------
// `golem:core/types@2.0.0` JS facade traits (wasm-rquickjs shape).
//
// Mirrors `golem_core_2_0_0_types.d.ts`: `{ tag, val }` tagged unions, `bigint`
// (`js.BigInt`) for s64/u64/quantity-mantissa/duration-ns/quota counters/
// datetime seconds, `Uint8Array` for binary bytes, `undefined` (`js.UndefOr`)
// for absent options, camelCase record fields. Lives in its own package so it
// does not collide with the `golem.host.js.JsUuid` etc. identifier facades.
//
// Tagged-union *bodies* (`JsSchemaTypeBody`, `JsSchemaValueNode`, `JsRole`,
// `JsDiscriminatorRule`, `JsResultValuePayload`) expose only `tag` here; their
// `val` payload is read positionally by `golem.host.SchemaWireInterop`, which
// owns the case <-> payload mapping.
// ---------------------------------------------------------------------------

// === Embedded common value records ===

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
sealed trait JsDatetime extends js.Object {
  def seconds: js.BigInt = js.native
  def nanoseconds: Int   = js.native
}
object JsDatetime {
  def apply(seconds: js.BigInt, nanoseconds: Int): JsDatetime =
    js.Dynamic.literal("seconds" -> seconds, "nanoseconds" -> nanoseconds).asInstanceOf[JsDatetime]
}

@js.native
sealed trait JsEnvironmentId extends js.Object {
  def uuid: JsUuid = js.native
}
object JsEnvironmentId {
  def apply(uuid: JsUuid): JsEnvironmentId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsEnvironmentId]
}

// === Role ===

@js.native
sealed trait JsRole extends js.Object {
  def tag: String = js.native
}
object JsRole {
  def multimodal: JsRole          = JsShape.tagOnly[JsRole]("multimodal")
  def other(name: String): JsRole = JsShape.tagged[JsRole]("other", name)
}

// === Metadata envelope ===

@js.native
sealed trait JsMetadataEnvelope extends js.Object {
  def doc: js.UndefOr[String]        = js.native
  def aliases: js.Array[String]      = js.native
  def examples: js.Array[String]     = js.native
  def deprecated: js.UndefOr[String] = js.native
  def role: js.UndefOr[JsRole]       = js.native
}
object JsMetadataEnvelope {
  def apply(
    doc: js.UndefOr[String],
    aliases: js.Array[String],
    examples: js.Array[String],
    deprecated: js.UndefOr[String],
    role: js.UndefOr[JsRole]
  ): JsMetadataEnvelope = {
    val o = js.Dynamic.literal("aliases" -> aliases, "examples" -> examples)
    doc.foreach(v => o.updateDynamic("doc")(v))
    deprecated.foreach(v => o.updateDynamic("deprecated")(v))
    role.foreach(v => o.updateDynamic("role")(v))
    o.asInstanceOf[JsMetadataEnvelope]
  }
}

// === Schema graph: defs / fields / cases ===

@js.native
sealed trait JsSchemaTypeDef extends js.Object {
  def id: String               = js.native
  def name: js.UndefOr[String] = js.native
  def body: Int                = js.native
}
object JsSchemaTypeDef {
  def apply(id: String, name: js.UndefOr[String], body: Int): JsSchemaTypeDef = {
    val o = js.Dynamic.literal("id" -> id, "body" -> body)
    name.foreach(v => o.updateDynamic("name")(v))
    o.asInstanceOf[JsSchemaTypeDef]
  }
}

@js.native
sealed trait JsNamedFieldType extends js.Object {
  def name: String                 = js.native
  def body: Int                    = js.native
  def metadata: JsMetadataEnvelope = js.native
}
object JsNamedFieldType {
  def apply(name: String, body: Int, metadata: JsMetadataEnvelope): JsNamedFieldType =
    js.Dynamic.literal("name" -> name, "body" -> body, "metadata" -> metadata).asInstanceOf[JsNamedFieldType]
}

@js.native
sealed trait JsVariantCaseType extends js.Object {
  def name: String                 = js.native
  def payload: js.UndefOr[Int]     = js.native
  def metadata: JsMetadataEnvelope = js.native
}
object JsVariantCaseType {
  def apply(name: String, payload: js.UndefOr[Int], metadata: JsMetadataEnvelope): JsVariantCaseType = {
    val o = js.Dynamic.literal("name" -> name, "metadata" -> metadata)
    payload.foreach(v => o.updateDynamic("payload")(v))
    o.asInstanceOf[JsVariantCaseType]
  }
}

// === Type-body specs ===

@js.native
sealed trait JsFixedListSpec extends js.Object {
  def element: Int = js.native
  def length: Int  = js.native
}
object JsFixedListSpec {
  def apply(element: Int, length: Int): JsFixedListSpec =
    js.Dynamic.literal("element" -> element, "length" -> length).asInstanceOf[JsFixedListSpec]
}

@js.native
sealed trait JsMapSpec extends js.Object {
  def key: Int   = js.native
  def value: Int = js.native
}
object JsMapSpec {
  def apply(key: Int, value: Int): JsMapSpec =
    js.Dynamic.literal("key" -> key, "value" -> value).asInstanceOf[JsMapSpec]
}

@js.native
sealed trait JsResultSpec extends js.Object {
  def ok: js.UndefOr[Int]  = js.native
  def err: js.UndefOr[Int] = js.native
}
object JsResultSpec {
  def apply(ok: js.UndefOr[Int], err: js.UndefOr[Int]): JsResultSpec = {
    val o = js.Dynamic.literal()
    ok.foreach(v => o.updateDynamic("ok")(v))
    err.foreach(v => o.updateDynamic("err")(v))
    o.asInstanceOf[JsResultSpec]
  }
}

@js.native
sealed trait JsNumericBound extends js.Object {
  def tag: String = js.native
}
object JsNumericBound {
  def signed(value: js.BigInt): JsNumericBound    = JsShape.tagged[JsNumericBound]("signed", value)
  def unsigned(value: js.BigInt): JsNumericBound  = JsShape.tagged[JsNumericBound]("unsigned", value)
  def floatBits(value: js.BigInt): JsNumericBound = JsShape.tagged[JsNumericBound]("float-bits", value)
}

@js.native
sealed trait JsNumericRestrictions extends js.Object {
  def min: js.UndefOr[JsNumericBound] = js.native
  def max: js.UndefOr[JsNumericBound] = js.native
  def unit: js.UndefOr[String]        = js.native
}
object JsNumericRestrictions {
  def apply(
    min: js.UndefOr[JsNumericBound],
    max: js.UndefOr[JsNumericBound],
    unit: js.UndefOr[String]
  ): JsNumericRestrictions = {
    val o = js.Dynamic.literal()
    min.foreach(v => o.updateDynamic("min")(v))
    max.foreach(v => o.updateDynamic("max")(v))
    unit.foreach(v => o.updateDynamic("unit")(v))
    o.asInstanceOf[JsNumericRestrictions]
  }
}

@js.native
sealed trait JsTextRestrictions extends js.Object {
  def languages: js.UndefOr[js.Array[String]] = js.native
  def minLength: js.UndefOr[Int]              = js.native
  def maxLength: js.UndefOr[Int]              = js.native
  def regex: js.UndefOr[String]               = js.native
}
object JsTextRestrictions {
  def apply(
    languages: js.UndefOr[js.Array[String]],
    minLength: js.UndefOr[Int],
    maxLength: js.UndefOr[Int],
    regex: js.UndefOr[String]
  ): JsTextRestrictions = {
    val o = js.Dynamic.literal()
    languages.foreach(v => o.updateDynamic("languages")(v))
    minLength.foreach(v => o.updateDynamic("minLength")(v))
    maxLength.foreach(v => o.updateDynamic("maxLength")(v))
    regex.foreach(v => o.updateDynamic("regex")(v))
    o.asInstanceOf[JsTextRestrictions]
  }
}

@js.native
sealed trait JsBinaryRestrictions extends js.Object {
  def mimeTypes: js.UndefOr[js.Array[String]] = js.native
  def minBytes: js.UndefOr[Int]               = js.native
  def maxBytes: js.UndefOr[Int]               = js.native
}
object JsBinaryRestrictions {
  def apply(
    mimeTypes: js.UndefOr[js.Array[String]],
    minBytes: js.UndefOr[Int],
    maxBytes: js.UndefOr[Int]
  ): JsBinaryRestrictions = {
    val o = js.Dynamic.literal()
    mimeTypes.foreach(v => o.updateDynamic("mimeTypes")(v))
    minBytes.foreach(v => o.updateDynamic("minBytes")(v))
    maxBytes.foreach(v => o.updateDynamic("maxBytes")(v))
    o.asInstanceOf[JsBinaryRestrictions]
  }
}

@js.native
sealed trait JsPathSpec extends js.Object {
  def direction: String                               = js.native
  def kind: String                                    = js.native
  def allowedMimeTypes: js.UndefOr[js.Array[String]]  = js.native
  def allowedExtensions: js.UndefOr[js.Array[String]] = js.native
}
object JsPathSpec {
  def apply(
    direction: String,
    kind: String,
    allowedMimeTypes: js.UndefOr[js.Array[String]],
    allowedExtensions: js.UndefOr[js.Array[String]]
  ): JsPathSpec = {
    val o = js.Dynamic.literal("direction" -> direction, "kind" -> kind)
    allowedMimeTypes.foreach(v => o.updateDynamic("allowedMimeTypes")(v))
    allowedExtensions.foreach(v => o.updateDynamic("allowedExtensions")(v))
    o.asInstanceOf[JsPathSpec]
  }
}

@js.native
sealed trait JsUrlRestrictions extends js.Object {
  def allowedSchemes: js.UndefOr[js.Array[String]] = js.native
  def allowedHosts: js.UndefOr[js.Array[String]]   = js.native
}
object JsUrlRestrictions {
  def apply(
    allowedSchemes: js.UndefOr[js.Array[String]],
    allowedHosts: js.UndefOr[js.Array[String]]
  ): JsUrlRestrictions = {
    val o = js.Dynamic.literal()
    allowedSchemes.foreach(v => o.updateDynamic("allowedSchemes")(v))
    allowedHosts.foreach(v => o.updateDynamic("allowedHosts")(v))
    o.asInstanceOf[JsUrlRestrictions]
  }
}

@js.native
sealed trait JsQuantityValue extends js.Object {
  def mantissa: js.BigInt = js.native
  def scale: Int          = js.native
  def unit: String        = js.native
}
object JsQuantityValue {
  def apply(mantissa: js.BigInt, scale: Int, unit: String): JsQuantityValue =
    js.Dynamic.literal("mantissa" -> mantissa, "scale" -> scale, "unit" -> unit).asInstanceOf[JsQuantityValue]
}

@js.native
sealed trait JsQuantitySpec extends js.Object {
  def baseUnit: String                  = js.native
  def allowedSuffixes: js.Array[String] = js.native
  def min: js.UndefOr[JsQuantityValue]  = js.native
  def max: js.UndefOr[JsQuantityValue]  = js.native
}
object JsQuantitySpec {
  def apply(
    baseUnit: String,
    allowedSuffixes: js.Array[String],
    min: js.UndefOr[JsQuantityValue],
    max: js.UndefOr[JsQuantityValue]
  ): JsQuantitySpec = {
    val o = js.Dynamic.literal("baseUnit" -> baseUnit, "allowedSuffixes" -> allowedSuffixes)
    min.foreach(v => o.updateDynamic("min")(v))
    max.foreach(v => o.updateDynamic("max")(v))
    o.asInstanceOf[JsQuantitySpec]
  }
}

// === Discriminated union spec ===

@js.native
sealed trait JsFieldDiscriminator extends js.Object {
  def fieldName: String           = js.native
  def literal: js.UndefOr[String] = js.native
}
object JsFieldDiscriminator {
  def apply(fieldName: String, literal: js.UndefOr[String]): JsFieldDiscriminator = {
    val o = js.Dynamic.literal("fieldName" -> fieldName)
    literal.foreach(v => o.updateDynamic("literal")(v))
    o.asInstanceOf[JsFieldDiscriminator]
  }
}

@js.native
sealed trait JsDiscriminatorRule extends js.Object {
  def tag: String = js.native
}
object JsDiscriminatorRule {
  def prefix(v: String): JsDiscriminatorRule                    = JsShape.tagged[JsDiscriminatorRule]("prefix", v)
  def suffix(v: String): JsDiscriminatorRule                    = JsShape.tagged[JsDiscriminatorRule]("suffix", v)
  def contains(v: String): JsDiscriminatorRule                  = JsShape.tagged[JsDiscriminatorRule]("contains", v)
  def regex(v: String): JsDiscriminatorRule                     = JsShape.tagged[JsDiscriminatorRule]("regex", v)
  def fieldEquals(v: JsFieldDiscriminator): JsDiscriminatorRule =
    JsShape.tagged[JsDiscriminatorRule]("field-equals", v)
  def fieldAbsent(v: String): JsDiscriminatorRule = JsShape.tagged[JsDiscriminatorRule]("field-absent", v)
}

@js.native
sealed trait JsUnionBranch extends js.Object {
  def tag: String                        = js.native
  def body: Int                          = js.native
  def discriminator: JsDiscriminatorRule = js.native
  def metadata: JsMetadataEnvelope       = js.native
}
object JsUnionBranch {
  def apply(tag: String, body: Int, discriminator: JsDiscriminatorRule, metadata: JsMetadataEnvelope): JsUnionBranch =
    js.Dynamic
      .literal("tag" -> tag, "body" -> body, "discriminator" -> discriminator, "metadata" -> metadata)
      .asInstanceOf[JsUnionBranch]
}

@js.native
sealed trait JsUnionSpec extends js.Object {
  def branches: js.Array[JsUnionBranch] = js.native
}
object JsUnionSpec {
  def apply(branches: js.Array[JsUnionBranch]): JsUnionSpec =
    js.Dynamic.literal("branches" -> branches).asInstanceOf[JsUnionSpec]
}

// === Capability specs ===

@js.native
sealed trait JsSecretSpec extends js.Object {
  def inner: Int                   = js.native
  def category: js.UndefOr[String] = js.native
}
object JsSecretSpec {
  def apply(inner: Int, category: js.UndefOr[String]): JsSecretSpec = {
    val o = js.Dynamic.literal("inner" -> inner)
    category.foreach(v => o.updateDynamic("category")(v))
    o.asInstanceOf[JsSecretSpec]
  }
}

@js.native
sealed trait JsQuotaTokenSpec extends js.Object {
  def resourceName: js.UndefOr[String] = js.native
}
object JsQuotaTokenSpec {
  def apply(resourceName: js.UndefOr[String]): JsQuotaTokenSpec = {
    val o = js.Dynamic.literal()
    resourceName.foreach(v => o.updateDynamic("resourceName")(v))
    o.asInstanceOf[JsQuotaTokenSpec]
  }
}

// === Schema type body / node / graph ===

@js.native
sealed trait JsSchemaTypeBody extends js.Object {
  def tag: String = js.native
}
object JsSchemaTypeBody {
  def refType(defIndex: Int): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("ref-type", defIndex)

  def boolType: JsSchemaTypeBody                                     = JsShape.tagOnly[JsSchemaTypeBody]("bool-type")
  def s8Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("s8-type", r)
  def s16Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("s16-type", r)
  def s32Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("s32-type", r)
  def s64Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("s64-type", r)
  def u8Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("u8-type", r)
  def u16Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("u16-type", r)
  def u32Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("u32-type", r)
  def u64Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("u64-type", r)
  def f32Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("f32-type", r)
  def f64Type(r: js.UndefOr[JsNumericRestrictions]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("f64-type", r)
  def charType: JsSchemaTypeBody   = JsShape.tagOnly[JsSchemaTypeBody]("char-type")
  def stringType: JsSchemaTypeBody = JsShape.tagOnly[JsSchemaTypeBody]("string-type")

  def recordType(fields: js.Array[JsNamedFieldType]): JsSchemaTypeBody =
    JsShape.tagged[JsSchemaTypeBody]("record-type", fields)
  def variantType(cases: js.Array[JsVariantCaseType]): JsSchemaTypeBody =
    JsShape.tagged[JsSchemaTypeBody]("variant-type", cases)
  def enumType(cases: js.Array[String]): JsSchemaTypeBody  = JsShape.tagged[JsSchemaTypeBody]("enum-type", cases)
  def flagsType(names: js.Array[String]): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("flags-type", names)
  def tupleType(elements: js.Array[Int]): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("tuple-type", elements)
  def listType(element: Int): JsSchemaTypeBody             =
    JsShape.tagged[JsSchemaTypeBody]("list-type", element.asInstanceOf[js.Any])
  def fixedListType(spec: JsFixedListSpec): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("fixed-list-type", spec)
  def mapType(spec: JsMapSpec): JsSchemaTypeBody             = JsShape.tagged[JsSchemaTypeBody]("map-type", spec)
  def optionType(element: Int): JsSchemaTypeBody             =
    JsShape.tagged[JsSchemaTypeBody]("option-type", element.asInstanceOf[js.Any])
  def resultType(spec: JsResultSpec): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("result-type", spec)

  def textType(r: JsTextRestrictions): JsSchemaTypeBody     = JsShape.tagged[JsSchemaTypeBody]("text-type", r)
  def binaryType(r: JsBinaryRestrictions): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("binary-type", r)
  def pathType(spec: JsPathSpec): JsSchemaTypeBody          = JsShape.tagged[JsSchemaTypeBody]("path-type", spec)
  def urlType(r: JsUrlRestrictions): JsSchemaTypeBody       = JsShape.tagged[JsSchemaTypeBody]("url-type", r)
  def datetimeType: JsSchemaTypeBody                        = JsShape.tagOnly[JsSchemaTypeBody]("datetime-type")
  def durationType: JsSchemaTypeBody                        = JsShape.tagOnly[JsSchemaTypeBody]("duration-type")
  def quantityType(spec: JsQuantitySpec): JsSchemaTypeBody  = JsShape.tagged[JsSchemaTypeBody]("quantity-type", spec)

  def unionType(spec: JsUnionSpec): JsSchemaTypeBody = JsShape.tagged[JsSchemaTypeBody]("union-type", spec)

  def secretType(spec: JsSecretSpec): JsSchemaTypeBody         = JsShape.tagged[JsSchemaTypeBody]("secret-type", spec)
  def quotaTokenType(spec: JsQuotaTokenSpec): JsSchemaTypeBody =
    JsShape.tagged[JsSchemaTypeBody]("quota-token-type", spec)

  def futureType(element: js.UndefOr[Int]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("future-type", element.map(_.asInstanceOf[js.Any]))
  def streamType(element: js.UndefOr[Int]): JsSchemaTypeBody =
    JsShape.taggedOptional[JsSchemaTypeBody]("stream-type", element.map(_.asInstanceOf[js.Any]))
}

@js.native
sealed trait JsSchemaTypeNode extends js.Object {
  def body: JsSchemaTypeBody       = js.native
  def metadata: JsMetadataEnvelope = js.native
}
object JsSchemaTypeNode {
  def apply(body: JsSchemaTypeBody, metadata: JsMetadataEnvelope): JsSchemaTypeNode =
    js.Dynamic.literal("body" -> body, "metadata" -> metadata).asInstanceOf[JsSchemaTypeNode]
}

@js.native
sealed trait JsSchemaGraph extends js.Object {
  def typeNodes: js.Array[JsSchemaTypeNode] = js.native
  def defs: js.Array[JsSchemaTypeDef]       = js.native
  def root: Int                             = js.native
}
object JsSchemaGraph {
  def apply(typeNodes: js.Array[JsSchemaTypeNode], defs: js.Array[JsSchemaTypeDef], root: Int): JsSchemaGraph =
    js.Dynamic.literal("typeNodes" -> typeNodes, "defs" -> defs, "root" -> root).asInstanceOf[JsSchemaGraph]
}

// === Schema value payloads ===

@js.native
sealed trait JsVariantValuePayload extends js.Object {
  @JSName("case_") def caseIndex: Int = js.native
  def payload: js.UndefOr[Int]        = js.native
}
object JsVariantValuePayload {
  def apply(caseIndex: Int, payload: js.UndefOr[Int]): JsVariantValuePayload = {
    val o = js.Dynamic.literal("case_" -> caseIndex)
    payload.foreach(v => o.updateDynamic("payload")(v))
    o.asInstanceOf[JsVariantValuePayload]
  }
}

@js.native
sealed trait JsMapEntry extends js.Object {
  def key: Int   = js.native
  def value: Int = js.native
}
object JsMapEntry {
  def apply(key: Int, value: Int): JsMapEntry =
    js.Dynamic.literal("key" -> key, "value" -> value).asInstanceOf[JsMapEntry]
}

@js.native
sealed trait JsResultValuePayload extends js.Object {
  def tag: String = js.native
}
object JsResultValuePayload {
  def okValue(value: js.UndefOr[Int]): JsResultValuePayload =
    JsShape.taggedOptional[JsResultValuePayload]("ok-value", value.map(_.asInstanceOf[js.Any]))
  def errValue(value: js.UndefOr[Int]): JsResultValuePayload =
    JsShape.taggedOptional[JsResultValuePayload]("err-value", value.map(_.asInstanceOf[js.Any]))
}

@js.native
sealed trait JsTextValuePayload extends js.Object {
  def text: String                 = js.native
  def language: js.UndefOr[String] = js.native
}
object JsTextValuePayload {
  def apply(text: String, language: js.UndefOr[String]): JsTextValuePayload = {
    val o = js.Dynamic.literal("text" -> text)
    language.foreach(v => o.updateDynamic("language")(v))
    o.asInstanceOf[JsTextValuePayload]
  }
}

@js.native
sealed trait JsBinaryValuePayload extends js.Object {
  def bytes: Uint8Array            = js.native
  def mimeType: js.UndefOr[String] = js.native
}
object JsBinaryValuePayload {
  def apply(bytes: Uint8Array, mimeType: js.UndefOr[String]): JsBinaryValuePayload = {
    val o = js.Dynamic.literal("bytes" -> bytes)
    mimeType.foreach(v => o.updateDynamic("mimeType")(v))
    o.asInstanceOf[JsBinaryValuePayload]
  }
}

@js.native
sealed trait JsDurationValuePayload extends js.Object {
  def nanoseconds: js.BigInt = js.native
}
object JsDurationValuePayload {
  def apply(nanoseconds: js.BigInt): JsDurationValuePayload =
    js.Dynamic.literal("nanoseconds" -> nanoseconds).asInstanceOf[JsDurationValuePayload]
}

@js.native
sealed trait JsUnionValuePayload extends js.Object {
  def tag: String = js.native
  def body: Int   = js.native
}
object JsUnionValuePayload {
  def apply(tag: String, body: Int): JsUnionValuePayload =
    js.Dynamic.literal("tag" -> tag, "body" -> body).asInstanceOf[JsUnionValuePayload]
}

// === Schema value node / tree / typed value ===

@js.native
sealed trait JsSchemaValueNode extends js.Object {
  def tag: String = js.native
}
object JsSchemaValueNode {
  def boolValue(v: Boolean): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("bool-value", v.asInstanceOf[js.Any])
  def s8Value(v: Int): JsSchemaValueNode        = JsShape.tagged[JsSchemaValueNode]("s8-value", v.asInstanceOf[js.Any])
  def s16Value(v: Int): JsSchemaValueNode       = JsShape.tagged[JsSchemaValueNode]("s16-value", v.asInstanceOf[js.Any])
  def s32Value(v: Int): JsSchemaValueNode       = JsShape.tagged[JsSchemaValueNode]("s32-value", v.asInstanceOf[js.Any])
  def s64Value(v: js.BigInt): JsSchemaValueNode = JsShape.tagged[JsSchemaValueNode]("s64-value", v)
  def u8Value(v: Int): JsSchemaValueNode        = JsShape.tagged[JsSchemaValueNode]("u8-value", v.asInstanceOf[js.Any])
  def u16Value(v: Int): JsSchemaValueNode       = JsShape.tagged[JsSchemaValueNode]("u16-value", v.asInstanceOf[js.Any])
  def u32Value(v: Double): JsSchemaValueNode    = JsShape.tagged[JsSchemaValueNode]("u32-value", v.asInstanceOf[js.Any])
  def u64Value(v: js.BigInt): JsSchemaValueNode = JsShape.tagged[JsSchemaValueNode]("u64-value", v)
  def f32Value(v: Float): JsSchemaValueNode     = JsShape.tagged[JsSchemaValueNode]("f32-value", v.asInstanceOf[js.Any])
  def f64Value(v: Double): JsSchemaValueNode    = JsShape.tagged[JsSchemaValueNode]("f64-value", v.asInstanceOf[js.Any])
  def charValue(v: String): JsSchemaValueNode   = JsShape.tagged[JsSchemaValueNode]("char-value", v)
  def stringValue(v: String): JsSchemaValueNode = JsShape.tagged[JsSchemaValueNode]("string-value", v)

  def recordValue(fields: js.Array[Int]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("record-value", fields)
  def variantValue(p: JsVariantValuePayload): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("variant-value", p)
  def enumValue(caseIndex: Int): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("enum-value", caseIndex.asInstanceOf[js.Any])
  def flagsValue(flags: js.Array[Boolean]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("flags-value", flags)
  def tupleValue(elements: js.Array[Int]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("tuple-value", elements)
  def listValue(elements: js.Array[Int]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("list-value", elements)
  def fixedListValue(elements: js.Array[Int]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("fixed-list-value", elements)
  def mapValue(entries: js.Array[JsMapEntry]): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("map-value", entries)
  def optionValue(value: js.UndefOr[Int]): JsSchemaValueNode =
    JsShape.taggedOptional[JsSchemaValueNode]("option-value", value.map(_.asInstanceOf[js.Any]))
  def resultValue(p: JsResultValuePayload): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("result-value", p)

  def textValue(p: JsTextValuePayload): JsSchemaValueNode         = JsShape.tagged[JsSchemaValueNode]("text-value", p)
  def binaryValue(p: JsBinaryValuePayload): JsSchemaValueNode     = JsShape.tagged[JsSchemaValueNode]("binary-value", p)
  def pathValue(v: String): JsSchemaValueNode                     = JsShape.tagged[JsSchemaValueNode]("path-value", v)
  def urlValue(v: String): JsSchemaValueNode                      = JsShape.tagged[JsSchemaValueNode]("url-value", v)
  def datetimeValue(v: JsDatetime): JsSchemaValueNode             = JsShape.tagged[JsSchemaValueNode]("datetime-value", v)
  def durationValue(p: JsDurationValuePayload): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("duration-value", p)
  def quantityValueNode(v: JsQuantityValue): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("quantity-value-node", v)

  def unionValue(p: JsUnionValuePayload): JsSchemaValueNode = JsShape.tagged[JsSchemaValueNode]("union-value", p)

  def secretValue(resource: js.Any): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("secret-value", resource)

  /**
   * `schema-value-node :: quota-token-handle(own<quota-token>)`. The `val`
   * carries the opaque owned `golem:core/types@2.0.0` `quota-token` resource as
   * an unforgeable handle; it has no readable structure.
   */
  def quotaTokenHandle(resource: js.Any): JsSchemaValueNode =
    JsShape.tagged[JsSchemaValueNode]("quota-token-handle", resource)
}

@js.native
sealed trait JsSchemaValueTree extends js.Object {
  def valueNodes: js.Array[JsSchemaValueNode] = js.native
  def root: Int                               = js.native
}
object JsSchemaValueTree {
  def apply(valueNodes: js.Array[JsSchemaValueNode], root: Int): JsSchemaValueTree =
    js.Dynamic.literal("valueNodes" -> valueNodes, "root" -> root).asInstanceOf[JsSchemaValueTree]
}

@js.native
sealed trait JsTypedSchemaValue extends js.Object {
  def graph: JsSchemaGraph     = js.native
  def value: JsSchemaValueTree = js.native
}
object JsTypedSchemaValue {
  def apply(graph: JsSchemaGraph, value: JsSchemaValueTree): JsTypedSchemaValue =
    js.Dynamic.literal("graph" -> graph, "value" -> value).asInstanceOf[JsTypedSchemaValue]
}
