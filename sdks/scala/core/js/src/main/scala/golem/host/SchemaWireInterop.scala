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

package golem.host

import golem.schema._
import golem.schema.wire._
import golem.host.js.schema._

import scala.collection.mutable
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Mechanical mapping between the host-agnostic flat carrier
 * [[golem.schema.wire]] (`Wit*`) and the `golem:core/types@2.0.0` JS facades
 * [[golem.host.js.schema]] (`Js*`).
 *
 * The recursive <-> flat conversion is owned by the shared `WireCodec` (Slice
 * 1); this layer is the pure, lossless `Wit* <-> Js*` rename. Width handling
 * mirrors the WIT: `u64` carries raw two's-complement bits in the `Wit*` model
 * and the unsigned value as a JS `bigint` (`golem.schema.U64`), `s64`/`s32`/
 * float/`u32` follow their natural JS shape, binary uses `Uint8Array`, and
 * `char` uses a JS code-point string.
 */
object SchemaWireInterop {

  // ===========================================================================
  // Public entry points
  // ===========================================================================

  def graphToJs(g: WitSchemaGraph): JsSchemaGraph =
    JsSchemaGraph(g.typeNodes.map(typeNodeToJs).toJSArray, g.defs.map(defToJs).toJSArray, g.root)

  def graphFromJs(j: JsSchemaGraph): WitSchemaGraph =
    WitSchemaGraph(j.typeNodes.toList.toVector.map(typeNodeFromJs), j.defs.toList.toVector.map(defFromJs), j.root)

  def valueTreeToJs(v: WitSchemaValueTree): JsSchemaValueTree = {
    preflightValueTree(v)
    JsSchemaValueTree(v.valueNodes.map(valueNodeToJs).toJSArray, v.root)
  }

  def valueTreeFromJs(j: JsSchemaValueTree): WitSchemaValueTree =
    WitSchemaValueTree(j.valueNodes.toList.toVector.map(valueNodeFromJs), j.root)

  def typedToJs(t: WitTypedSchemaValue): JsTypedSchemaValue =
    JsTypedSchemaValue(graphToJs(t.graph), valueTreeToJs(t.value))

  def typedFromJs(j: JsTypedSchemaValue): WitTypedSchemaValue =
    WitTypedSchemaValue(graphFromJs(j.graph), valueTreeFromJs(j.value))

  /**
   * Convert a model metadata envelope to its JS facade (used by the agent-type
   * encoder).
   */
  def metadataToJs(m: MetadataEnvelope): JsMetadataEnvelope = mdToJs(m)

  /** Convert a JS metadata envelope facade back to the model. */
  def metadataFromJs(j: JsMetadataEnvelope): MetadataEnvelope = mdFromJs(j)

  // ===========================================================================
  // Low-level helpers
  // ===========================================================================

  /** Read the positional `val` payload of a `{ tag, val }` JS object. */
  private def valOf(o: js.Object): js.Dynamic =
    o.asInstanceOf[js.Dynamic].selectDynamic("val")

  /** Read an optional index `val` (absent => `None`). */
  private def optIntVal(o: js.Object): Option[Int] =
    o.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.UndefOr[Int]].toOption

  private def bytesToJs(bytes: Vector[Byte]): Uint8Array = {
    val arr = new Uint8Array(bytes.length)
    var i   = 0
    while (i < bytes.length) {
      arr(i) = (bytes(i) & 0xff).toShort
      i += 1
    }
    arr
  }

  private def bytesFromJs(arr: Uint8Array): Vector[Byte] = {
    val b = Vector.newBuilder[Byte]
    var i = 0
    while (i < arr.length) {
      b += arr(i).toByte
      i += 1
    }
    b.result()
  }

  // ===========================================================================
  // Embedded common records
  // ===========================================================================

  private def datetimeToJs(d: Datetime): JsDatetime   = JsDatetime(js.BigInt(d.seconds.toString), d.nanoseconds)
  private def datetimeFromJs(j: JsDatetime): Datetime = Datetime(BigInt(j.seconds.toString).toLong, j.nanoseconds)

  // ===========================================================================
  // Metadata
  // ===========================================================================

  private def roleToJs(r: Role): JsRole =
    r match {
      case Role.Multimodal  => JsRole.multimodal
      case Role.Other(name) => JsRole.other(name)
    }

  private def roleFromJs(j: JsRole): Role =
    j.tag match {
      case "multimodal" => Role.Multimodal
      case "other"      => Role.Other(valOf(j).asInstanceOf[String])
      case other        => throw new IllegalArgumentException(s"Unknown role tag: $other")
    }

  private def mdToJs(m: MetadataEnvelope): JsMetadataEnvelope =
    JsMetadataEnvelope(
      m.doc.orUndefined,
      m.aliases.toJSArray,
      m.examples.toJSArray,
      m.deprecated.orUndefined,
      m.role.map(roleToJs).orUndefined
    )

  private def mdFromJs(j: JsMetadataEnvelope): MetadataEnvelope =
    MetadataEnvelope(
      j.doc.toOption,
      j.aliases.toList,
      j.examples.toList,
      j.deprecated.toOption,
      j.role.toOption.map(roleFromJs)
    )

  // ===========================================================================
  // Specs
  // ===========================================================================

  private def textRToJs(t: TextRestrictions): JsTextRestrictions =
    JsTextRestrictions(
      t.languages.map(_.toJSArray).orUndefined,
      t.minLength.orUndefined,
      t.maxLength.orUndefined,
      t.regex.orUndefined
    )

  private def textRFromJs(j: JsTextRestrictions): TextRestrictions =
    TextRestrictions(j.languages.toOption.map(_.toList), j.minLength.toOption, j.maxLength.toOption, j.regex.toOption)

  private def binRToJs(b: BinaryRestrictions): JsBinaryRestrictions =
    JsBinaryRestrictions(b.mimeTypes.map(_.toJSArray).orUndefined, b.minBytes.orUndefined, b.maxBytes.orUndefined)

  private def binRFromJs(j: JsBinaryRestrictions): BinaryRestrictions =
    BinaryRestrictions(j.mimeTypes.toOption.map(_.toList), j.minBytes.toOption, j.maxBytes.toOption)

  private def pathDirToStr(d: PathDirection): String =
    d match {
      case PathDirection.Input  => "input"
      case PathDirection.Output => "output"
      case PathDirection.InOut  => "in-out"
    }

  private def pathDirFromStr(s: String): PathDirection =
    s match {
      case "input"  => PathDirection.Input
      case "output" => PathDirection.Output
      case "in-out" => PathDirection.InOut
      case other    => throw new IllegalArgumentException(s"Unknown path direction: $other")
    }

  private def pathKindToStr(k: PathKind): String =
    k match {
      case PathKind.File      => "file"
      case PathKind.Directory => "directory"
      case PathKind.Any       => "any"
    }

  private def pathKindFromStr(s: String): PathKind =
    s match {
      case "file"      => PathKind.File
      case "directory" => PathKind.Directory
      case "any"       => PathKind.Any
      case other       => throw new IllegalArgumentException(s"Unknown path kind: $other")
    }

  private def pathToJs(p: PathSpec): JsPathSpec =
    JsPathSpec(
      pathDirToStr(p.direction),
      pathKindToStr(p.kind),
      p.allowedMimeTypes.map(_.toJSArray).orUndefined,
      p.allowedExtensions.map(_.toJSArray).orUndefined
    )

  private def pathFromJs(j: JsPathSpec): PathSpec =
    PathSpec(
      pathDirFromStr(j.direction),
      pathKindFromStr(j.kind),
      j.allowedMimeTypes.toOption.map(_.toList),
      j.allowedExtensions.toOption.map(_.toList)
    )

  private def urlRToJs(u: UrlRestrictions): JsUrlRestrictions =
    JsUrlRestrictions(u.allowedSchemes.map(_.toJSArray).orUndefined, u.allowedHosts.map(_.toJSArray).orUndefined)

  private def urlRFromJs(j: JsUrlRestrictions): UrlRestrictions =
    UrlRestrictions(j.allowedSchemes.toOption.map(_.toList), j.allowedHosts.toOption.map(_.toList))

  private def quantityValueToJs(q: QuantityValue): JsQuantityValue =
    JsQuantityValue(js.BigInt(q.mantissa.toString), q.scale, q.unit)

  private def quantityValueFromJs(j: JsQuantityValue): QuantityValue =
    QuantityValue(BigInt(j.mantissa.toString).toLong, j.scale, j.unit)

  private def quantToJs(q: QuantitySpec): JsQuantitySpec =
    JsQuantitySpec(
      q.baseUnit,
      q.allowedSuffixes.toJSArray,
      q.min.map(quantityValueToJs).orUndefined,
      q.max.map(quantityValueToJs).orUndefined
    )

  private def quantFromJs(j: JsQuantitySpec): QuantitySpec =
    QuantitySpec(
      j.baseUnit,
      j.allowedSuffixes.toList,
      j.min.toOption.map(quantityValueFromJs),
      j.max.toOption.map(quantityValueFromJs)
    )

  private def fieldDiscToJs(f: FieldDiscriminator): JsFieldDiscriminator =
    JsFieldDiscriminator(f.fieldName, f.literal.orUndefined)

  private def fieldDiscFromJs(j: JsFieldDiscriminator): FieldDiscriminator =
    FieldDiscriminator(j.fieldName, j.literal.toOption)

  private def discToJs(d: DiscriminatorRule): JsDiscriminatorRule =
    d match {
      case DiscriminatorRule.Prefix(v)      => JsDiscriminatorRule.prefix(v)
      case DiscriminatorRule.Suffix(v)      => JsDiscriminatorRule.suffix(v)
      case DiscriminatorRule.Contains(v)    => JsDiscriminatorRule.contains(v)
      case DiscriminatorRule.Regex(v)       => JsDiscriminatorRule.regex(v)
      case DiscriminatorRule.FieldEquals(f) => JsDiscriminatorRule.fieldEquals(fieldDiscToJs(f))
      case DiscriminatorRule.FieldAbsent(v) => JsDiscriminatorRule.fieldAbsent(v)
    }

  private def discFromJs(j: JsDiscriminatorRule): DiscriminatorRule =
    j.tag match {
      case "prefix"       => DiscriminatorRule.Prefix(valOf(j).asInstanceOf[String])
      case "suffix"       => DiscriminatorRule.Suffix(valOf(j).asInstanceOf[String])
      case "contains"     => DiscriminatorRule.Contains(valOf(j).asInstanceOf[String])
      case "regex"        => DiscriminatorRule.Regex(valOf(j).asInstanceOf[String])
      case "field-equals" => DiscriminatorRule.FieldEquals(fieldDiscFromJs(valOf(j).asInstanceOf[JsFieldDiscriminator]))
      case "field-absent" => DiscriminatorRule.FieldAbsent(valOf(j).asInstanceOf[String])
      case other          => throw new IllegalArgumentException(s"Unknown discriminator tag: $other")
    }

  private def secretSpecToJs(s: WitSecretSpec): JsSecretSpec   = JsSecretSpec(s.inner, s.category.orUndefined)
  private def secretSpecFromJs(j: JsSecretSpec): WitSecretSpec = WitSecretSpec(j.inner, j.category.toOption)

  private def quotaSpecToJs(s: QuotaTokenSpec): JsQuotaTokenSpec   = JsQuotaTokenSpec(s.resourceName.orUndefined)
  private def quotaSpecFromJs(j: JsQuotaTokenSpec): QuotaTokenSpec = QuotaTokenSpec(j.resourceName.toOption)

  // ===========================================================================
  // Schema graph: defs / fields / cases / type body / node
  // ===========================================================================

  private def defToJs(d: WitSchemaTypeDef): JsSchemaTypeDef   = JsSchemaTypeDef(d.id, d.name.orUndefined, d.body)
  private def defFromJs(j: JsSchemaTypeDef): WitSchemaTypeDef = WitSchemaTypeDef(j.id, j.name.toOption, j.body)

  private def namedFieldToJs(f: WitNamedFieldType): JsNamedFieldType =
    JsNamedFieldType(f.name, f.body, mdToJs(f.metadata))

  private def namedFieldFromJs(j: JsNamedFieldType): WitNamedFieldType =
    WitNamedFieldType(j.name, j.body, mdFromJs(j.metadata))

  private def variantCaseToJs(c: WitVariantCaseType): JsVariantCaseType =
    JsVariantCaseType(c.name, c.payload.orUndefined, mdToJs(c.metadata))

  private def variantCaseFromJs(j: JsVariantCaseType): WitVariantCaseType =
    WitVariantCaseType(j.name, j.payload.toOption, mdFromJs(j.metadata))

  private def resultSpecToJs(s: WitResultSpec): JsResultSpec   = JsResultSpec(s.ok.orUndefined, s.err.orUndefined)
  private def resultSpecFromJs(j: JsResultSpec): WitResultSpec = WitResultSpec(j.ok.toOption, j.err.toOption)

  private def unionBranchToJs(b: WitUnionBranch): JsUnionBranch =
    JsUnionBranch(b.tag, b.body, discToJs(b.discriminator), mdToJs(b.metadata))

  private def unionBranchFromJs(j: JsUnionBranch): WitUnionBranch =
    WitUnionBranch(j.tag, j.body, discFromJs(j.discriminator), mdFromJs(j.metadata))

  private def unionSpecToJs(s: WitUnionSpec): JsUnionSpec   = JsUnionSpec(s.branches.map(unionBranchToJs).toJSArray)
  private def unionSpecFromJs(j: JsUnionSpec): WitUnionSpec =
    WitUnionSpec(j.branches.toList.toVector.map(unionBranchFromJs))

  private def typeNodeToJs(n: WitSchemaTypeNode): JsSchemaTypeNode =
    JsSchemaTypeNode(typeBodyToJs(n.body), mdToJs(n.metadata))

  private def typeNodeFromJs(j: JsSchemaTypeNode): WitSchemaTypeNode =
    WitSchemaTypeNode(typeBodyFromJs(j.body), mdFromJs(j.metadata))

  private def typeBodyToJs(b: WitSchemaTypeBody): JsSchemaTypeBody = {
    import WitSchemaTypeBody._
    b match {
      case RefType(i)        => JsSchemaTypeBody.refType(i)
      case BoolType          => JsSchemaTypeBody.boolType
      case S8Type            => JsSchemaTypeBody.s8Type
      case S16Type           => JsSchemaTypeBody.s16Type
      case S32Type           => JsSchemaTypeBody.s32Type
      case S64Type           => JsSchemaTypeBody.s64Type
      case U8Type            => JsSchemaTypeBody.u8Type
      case U16Type           => JsSchemaTypeBody.u16Type
      case U32Type           => JsSchemaTypeBody.u32Type
      case U64Type           => JsSchemaTypeBody.u64Type
      case F32Type           => JsSchemaTypeBody.f32Type
      case F64Type           => JsSchemaTypeBody.f64Type
      case CharType          => JsSchemaTypeBody.charType
      case StringType        => JsSchemaTypeBody.stringType
      case RecordType(fs)    => JsSchemaTypeBody.recordType(fs.map(namedFieldToJs).toJSArray)
      case VariantType(cs)   => JsSchemaTypeBody.variantType(cs.map(variantCaseToJs).toJSArray)
      case EnumType(cs)      => JsSchemaTypeBody.enumType(cs.toJSArray)
      case FlagsType(ns)     => JsSchemaTypeBody.flagsType(ns.toJSArray)
      case TupleType(es)     => JsSchemaTypeBody.tupleType(es.toJSArray)
      case ListType(e)       => JsSchemaTypeBody.listType(e)
      case FixedListType(s)  => JsSchemaTypeBody.fixedListType(JsFixedListSpec(s.element, s.length))
      case MapType(s)        => JsSchemaTypeBody.mapType(JsMapSpec(s.key, s.value))
      case OptionType(e)     => JsSchemaTypeBody.optionType(e)
      case ResultType(s)     => JsSchemaTypeBody.resultType(resultSpecToJs(s))
      case TextType(r)       => JsSchemaTypeBody.textType(textRToJs(r))
      case BinaryType(r)     => JsSchemaTypeBody.binaryType(binRToJs(r))
      case PathType(s)       => JsSchemaTypeBody.pathType(pathToJs(s))
      case UrlType(r)        => JsSchemaTypeBody.urlType(urlRToJs(r))
      case DatetimeType      => JsSchemaTypeBody.datetimeType
      case DurationType      => JsSchemaTypeBody.durationType
      case QuantityType(s)   => JsSchemaTypeBody.quantityType(quantToJs(s))
      case UnionType(s)      => JsSchemaTypeBody.unionType(unionSpecToJs(s))
      case SecretType(s)     => JsSchemaTypeBody.secretType(secretSpecToJs(s))
      case QuotaTokenType(s) => JsSchemaTypeBody.quotaTokenType(quotaSpecToJs(s))
      case FutureType(e)     => JsSchemaTypeBody.futureType(e.orUndefined)
      case StreamType(e)     => JsSchemaTypeBody.streamType(e.orUndefined)
    }
  }

  private def typeBodyFromJs(j: JsSchemaTypeBody): WitSchemaTypeBody = {
    import WitSchemaTypeBody._
    j.tag match {
      case "ref-type"    => RefType(valOf(j).asInstanceOf[Int])
      case "bool-type"   => BoolType
      case "s8-type"     => S8Type
      case "s16-type"    => S16Type
      case "s32-type"    => S32Type
      case "s64-type"    => S64Type
      case "u8-type"     => U8Type
      case "u16-type"    => U16Type
      case "u32-type"    => U32Type
      case "u64-type"    => U64Type
      case "f32-type"    => F32Type
      case "f64-type"    => F64Type
      case "char-type"   => CharType
      case "string-type" => StringType
      case "record-type" =>
        RecordType(valOf(j).asInstanceOf[js.Array[JsNamedFieldType]].toList.toVector.map(namedFieldFromJs))
      case "variant-type" =>
        VariantType(valOf(j).asInstanceOf[js.Array[JsVariantCaseType]].toList.toVector.map(variantCaseFromJs))
      case "enum-type"       => EnumType(valOf(j).asInstanceOf[js.Array[String]].toList.toVector)
      case "flags-type"      => FlagsType(valOf(j).asInstanceOf[js.Array[String]].toList.toVector)
      case "tuple-type"      => TupleType(valOf(j).asInstanceOf[js.Array[Int]].toList.toVector)
      case "list-type"       => ListType(valOf(j).asInstanceOf[Int])
      case "fixed-list-type" =>
        val s = valOf(j).asInstanceOf[JsFixedListSpec]
        FixedListType(WitFixedListSpec(s.element, s.length))
      case "map-type" =>
        val s = valOf(j).asInstanceOf[JsMapSpec]
        MapType(WitMapSpec(s.key, s.value))
      case "option-type"      => OptionType(valOf(j).asInstanceOf[Int])
      case "result-type"      => ResultType(resultSpecFromJs(valOf(j).asInstanceOf[JsResultSpec]))
      case "text-type"        => TextType(textRFromJs(valOf(j).asInstanceOf[JsTextRestrictions]))
      case "binary-type"      => BinaryType(binRFromJs(valOf(j).asInstanceOf[JsBinaryRestrictions]))
      case "path-type"        => PathType(pathFromJs(valOf(j).asInstanceOf[JsPathSpec]))
      case "url-type"         => UrlType(urlRFromJs(valOf(j).asInstanceOf[JsUrlRestrictions]))
      case "datetime-type"    => DatetimeType
      case "duration-type"    => DurationType
      case "quantity-type"    => QuantityType(quantFromJs(valOf(j).asInstanceOf[JsQuantitySpec]))
      case "union-type"       => UnionType(unionSpecFromJs(valOf(j).asInstanceOf[JsUnionSpec]))
      case "secret-type"      => SecretType(secretSpecFromJs(valOf(j).asInstanceOf[JsSecretSpec]))
      case "quota-token-type" => QuotaTokenType(quotaSpecFromJs(valOf(j).asInstanceOf[JsQuotaTokenSpec]))
      case "future-type"      => FutureType(optIntVal(j))
      case "stream-type"      => StreamType(optIntVal(j))
      case other              => throw new IllegalArgumentException(s"Unknown schema-type-body tag: $other")
    }
  }

  // ===========================================================================
  // Schema value nodes
  // ===========================================================================

  private def resultValuePayloadToJs(p: WitResultValuePayload): JsResultValuePayload =
    p match {
      case WitResultValuePayload.OkValue(o)  => JsResultValuePayload.okValue(o.orUndefined)
      case WitResultValuePayload.ErrValue(o) => JsResultValuePayload.errValue(o.orUndefined)
    }

  private def resultValuePayloadFromJs(j: JsResultValuePayload): WitResultValuePayload =
    j.tag match {
      case "ok-value"  => WitResultValuePayload.OkValue(optIntVal(j))
      case "err-value" => WitResultValuePayload.ErrValue(optIntVal(j))
      case other       => throw new IllegalArgumentException(s"Unknown result-value-payload tag: $other")
    }

  /**
   * Validate an entire `WitSchemaValueTree` before [[valueTreeToJs]] moves any
   * owned `quota-token` handle.
   *
   * `valueNodeToJs` performs the affine `handle.take()` in the same `map` pass
   * that also runs conversions which (here or at the component-model boundary
   * the resulting JS tree crosses next) can still reject a sibling: an invalid
   * `char` code point, an out-of-range `u8`/`u16`/`u32`, or an invalid
   * `datetime`. Without this preflight a sibling failure after a handle was
   * taken would silently destroy the token. Running this borrow-only validation
   * first keeps the move atomic: a tree the boundary would reject is rejected
   * before any handle leaves its cell.
   *
   * The check is a flat pass over every node, mirroring the `map` below (which
   * converts — and so takes — each array slot exactly once, regardless of how
   * the nodes reference each other). It deliberately does not validate child
   * indices: `valueNodeToJs` copies them verbatim without dereferencing, so a
   * dangling index cannot strand a taken handle, and several shape tests rely
   * on isolating a single node. Handles are deduplicated by the identity of the
   * underlying owned resource (peeked without consuming), so two distinct
   * holders wrapping the same raw `quota-token` are rejected too, not only the
   * same holder used twice.
   */
  private def preflightValueTree(v: WitSchemaValueTree): Unit = {
    import WitSchemaValueNode._
    val seenRawSecret = mutable.Set.empty[Any]
    val seenRawQuota  = mutable.Set.empty[Any]

    def checkRange(name: String, value: Long, min: Long, max: Long): Unit =
      if (value < min || value > max)
        throw SchemaEncodeError(s"$name value out of range: $value")

    v.valueNodes.foreach {
      case U8Value(value)   => checkRange("u8", value.toLong, 0L, 255L)
      case U16Value(value)  => checkRange("u16", value.toLong, 0L, 65535L)
      case U32Value(value)  => checkRange("u32", value, 0L, 4294967295L)
      case CharValue(value) =>
        // A WIT `char` is a Unicode scalar value; reject anything outside
        // `[0, 0x10FFFF]` or a lone surrogate, both of which the boundary
        // rejects (and which `Character.toChars` would otherwise mishandle).
        if (!Character.isValidCodePoint(value) || (value >= 0xd800 && value <= 0xdfff))
          throw SchemaEncodeError(s"char value is not a Unicode scalar value: $value")
      case DatetimeValue(dt) =>
        if (dt.nanoseconds < 0 || dt.nanoseconds >= 1000000000)
          throw SchemaEncodeError(
            s"invalid datetime value: nanoseconds must be in [0, 1_000_000_000), got ${dt.nanoseconds}"
          )
      case SecretValue(h) =>
        val raw = h
          .withHandle(identity)
          .getOrElse(
            throw SchemaEncodeError("secret handle was already transferred; an owned secret can only be sent once")
          )
        if (!seenRawSecret.add(raw))
          throw SchemaEncodeError("the same secret handle appeared more than once in one value tree")
      case QuotaTokenHandle(h) =>
        // Peek the underlying owned resource without consuming it so two distinct
        // holders wrapping the same raw handle are also rejected.
        val raw = h
          .withHandle(identity)
          .getOrElse(
            throw SchemaEncodeError(
              "quota-token handle was already transferred; an owned quota-token can only be sent once"
            )
          )
        if (!seenRawQuota.add(raw))
          throw SchemaEncodeError("the same quota-token handle appeared more than once in one value tree")
      case _ => ()
    }
  }

  private def valueNodeToJs(n: WitSchemaValueNode): JsSchemaValueNode = {
    import WitSchemaValueNode._
    n match {
      case BoolValue(v)       => JsSchemaValueNode.boolValue(v)
      case S8Value(v)         => JsSchemaValueNode.s8Value(v.toInt)
      case S16Value(v)        => JsSchemaValueNode.s16Value(v.toInt)
      case S32Value(v)        => JsSchemaValueNode.s32Value(v)
      case S64Value(v)        => JsSchemaValueNode.s64Value(js.BigInt(v.toString))
      case U8Value(v)         => JsSchemaValueNode.u8Value(v)
      case U16Value(v)        => JsSchemaValueNode.u16Value(v)
      case U32Value(v)        => JsSchemaValueNode.u32Value(v.toDouble)
      case U64Value(v)        => JsSchemaValueNode.u64Value(js.BigInt(U64.fromRawBits(v).toString))
      case F32Value(v)        => JsSchemaValueNode.f32Value(v)
      case F64Value(v)        => JsSchemaValueNode.f64Value(v)
      case CharValue(v)       => JsSchemaValueNode.charValue(new String(Character.toChars(v)))
      case StringValue(v)     => JsSchemaValueNode.stringValue(v)
      case RecordValue(fs)    => JsSchemaValueNode.recordValue(fs.toJSArray)
      case VariantValue(p)    => JsSchemaValueNode.variantValue(JsVariantValuePayload(p.caseIndex, p.payload.orUndefined))
      case EnumValue(ci)      => JsSchemaValueNode.enumValue(ci)
      case FlagsValue(fs)     => JsSchemaValueNode.flagsValue(fs.toJSArray)
      case TupleValue(es)     => JsSchemaValueNode.tupleValue(es.toJSArray)
      case ListValue(es)      => JsSchemaValueNode.listValue(es.toJSArray)
      case FixedListValue(es) => JsSchemaValueNode.fixedListValue(es.toJSArray)
      case MapValue(es)       => JsSchemaValueNode.mapValue(es.map(e => JsMapEntry(e.key, e.value)).toJSArray)
      case OptionValue(o)     => JsSchemaValueNode.optionValue(o.orUndefined)
      case ResultValue(p)     => JsSchemaValueNode.resultValue(resultValuePayloadToJs(p))
      case TextValue(p)       => JsSchemaValueNode.textValue(JsTextValuePayload(p.text, p.language.orUndefined))
      case BinaryValue(p)     =>
        JsSchemaValueNode.binaryValue(JsBinaryValuePayload(bytesToJs(p.bytes), p.mimeType.orUndefined))
      case PathValue(v)     => JsSchemaValueNode.pathValue(v)
      case UrlValue(v)      => JsSchemaValueNode.urlValue(v)
      case DatetimeValue(v) => JsSchemaValueNode.datetimeValue(datetimeToJs(v))
      case DurationValue(p) =>
        JsSchemaValueNode.durationValue(JsDurationValuePayload(js.BigInt(p.nanoseconds.toString)))
      case QuantityValueNode(v) => JsSchemaValueNode.quantityValueNode(quantityValueToJs(v))
      case UnionValue(p)        => JsSchemaValueNode.unionValue(JsUnionValuePayload(p.tag, p.body))
      case SecretValue(h)       =>
        h.take() match {
          case Some(raw) => JsSchemaValueNode.secretValue(raw.asInstanceOf[js.Any])
          case None      =>
            throw new IllegalStateException(
              "secret handle was already transferred; an owned secret can only be sent once"
            )
        }
      case QuotaTokenHandle(h) =>
        // Move the owned `quota-token` resource out of the opaque handle exactly
        // once. `schemaValueToWit` preflights that every handle is present and
        // unique, so this take always succeeds for a well-formed tree.
        h.take() match {
          case Some(raw) => JsSchemaValueNode.quotaTokenHandle(raw.asInstanceOf[js.Any])
          case None      =>
            throw new IllegalStateException(
              "quota-token handle was already transferred; an owned quota-token can only be sent once"
            )
        }
    }
  }

  private def valueNodeFromJs(j: JsSchemaValueNode): WitSchemaValueNode = {
    import WitSchemaValueNode._
    j.tag match {
      case "bool-value"    => BoolValue(valOf(j).asInstanceOf[Boolean])
      case "s8-value"      => S8Value(valOf(j).asInstanceOf[Int].toByte)
      case "s16-value"     => S16Value(valOf(j).asInstanceOf[Int].toShort)
      case "s32-value"     => S32Value(valOf(j).asInstanceOf[Int])
      case "s64-value"     => S64Value(BigInt(valOf(j).asInstanceOf[js.BigInt].toString).toLong)
      case "u8-value"      => U8Value(valOf(j).asInstanceOf[Int])
      case "u16-value"     => U16Value(valOf(j).asInstanceOf[Int])
      case "u32-value"     => U32Value(valOf(j).asInstanceOf[Double].toLong)
      case "u64-value"     => U64Value(U64.toRawBits(BigInt(valOf(j).asInstanceOf[js.BigInt].toString)))
      case "f32-value"     => F32Value(valOf(j).asInstanceOf[Float])
      case "f64-value"     => F64Value(valOf(j).asInstanceOf[Double])
      case "char-value"    => CharValue(valOf(j).asInstanceOf[String].codePointAt(0))
      case "string-value"  => StringValue(valOf(j).asInstanceOf[String])
      case "record-value"  => RecordValue(valOf(j).asInstanceOf[js.Array[Int]].toList.toVector)
      case "variant-value" =>
        val p = valOf(j).asInstanceOf[JsVariantValuePayload]
        VariantValue(WitVariantValuePayload(p.caseIndex, p.payload.toOption))
      case "enum-value"       => EnumValue(valOf(j).asInstanceOf[Int])
      case "flags-value"      => FlagsValue(valOf(j).asInstanceOf[js.Array[Boolean]].toList.toVector)
      case "tuple-value"      => TupleValue(valOf(j).asInstanceOf[js.Array[Int]].toList.toVector)
      case "list-value"       => ListValue(valOf(j).asInstanceOf[js.Array[Int]].toList.toVector)
      case "fixed-list-value" => FixedListValue(valOf(j).asInstanceOf[js.Array[Int]].toList.toVector)
      case "map-value"        =>
        MapValue(
          valOf(j).asInstanceOf[js.Array[JsMapEntry]].toList.toVector.map(e => WitMapEntry(e.key, e.value))
        )
      case "option-value" => OptionValue(optIntVal(j))
      case "result-value" => ResultValue(resultValuePayloadFromJs(valOf(j).asInstanceOf[JsResultValuePayload]))
      case "text-value"   =>
        val p = valOf(j).asInstanceOf[JsTextValuePayload]
        TextValue(WitTextValuePayload(p.text, p.language.toOption))
      case "binary-value" =>
        val p = valOf(j).asInstanceOf[JsBinaryValuePayload]
        BinaryValue(WitBinaryValuePayload(bytesFromJs(p.bytes), p.mimeType.toOption))
      case "path-value"     => PathValue(valOf(j).asInstanceOf[String])
      case "url-value"      => UrlValue(valOf(j).asInstanceOf[String])
      case "datetime-value" => DatetimeValue(datetimeFromJs(valOf(j).asInstanceOf[JsDatetime]))
      case "duration-value" =>
        val p = valOf(j).asInstanceOf[JsDurationValuePayload]
        DurationValue(WitDurationValuePayload(BigInt(p.nanoseconds.toString).toLong))
      case "quantity-value-node" => QuantityValueNode(quantityValueFromJs(valOf(j).asInstanceOf[JsQuantityValue]))
      case "union-value"         =>
        val p = valOf(j).asInstanceOf[JsUnionValuePayload]
        UnionValue(WitUnionValuePayload(p.tag, p.body))
      case "secret-value" =>
        SecretValue(GuestSecretHandle.fromRaw(valOf(j)))
      case "quota-token-handle" =>
        // Wrap the owned `quota-token` resource in a fresh take-once handle.
        QuotaTokenHandle(GuestQuotaTokenHandle.fromRaw(valOf(j)))
      case other => throw new IllegalArgumentException(s"Unknown schema-value-node tag: $other")
    }
  }
}
