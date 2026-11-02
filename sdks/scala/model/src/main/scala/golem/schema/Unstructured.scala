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

package golem.schema

import golem.schema.wire.*
import zio.blocks.chunk.Chunk
import zio.blocks.schema.{DynamicValue => DV, Modifier, PrimitiveValue, Schema}

import scala.annotation.publicInBinary
import scala.collection.immutable.ListMap

/**
 * Unstructured text: either inline text with an optional language code, or a
 * URL that refers to the text.
 *
 * The type parameter `L` is the allow-list of language codes:
 *   - `UnstructuredText[String]` accepts any language code;
 *   - `UnstructuredText["en" | "de"]` accepts only the listed codes, and the
 *     inline `languageCode` is typed accordingly.
 *
 * The schema is `variant { inline: text, url: url }` with the
 * `unstructured-text` role. When an agent method returns this type (also inside
 * `Option` or on the `Right` side of `Either`), the HTTP layer renders the
 * inline text as the raw response body.
 */
sealed trait UnstructuredText[L] extends Product with Serializable

object UnstructuredText {
  final case class Url[L](url: String)                                     extends UnstructuredText[L]
  final case class Inline[L](text: String, languageCode: Option[L] = None) extends UnstructuredText[L]

  def fromUrl[L](url: String): UnstructuredText[L]                      = Url(url)
  def fromInline[L](text: String): UnstructuredText[L]                  = Inline(text, None)
  def fromInline[L](text: String, languageCode: L): UnstructuredText[L] = Inline(text, Some(languageCode))

  implicit def schema[L](implicit allowed: AllowedLanguages[L]): Schema[UnstructuredText[L]] =
    Schema[DV]
      .transform[UnstructuredText[L]](
        dv =>
          Unstructured.validateText(Unstructured.textFromDynamic(dv), allowed.values).asInstanceOf[UnstructuredText[L]],
        Unstructured.textToDynamic
      )
      .modifiers(Unstructured.restrictionModifier(allowed.values).toList)

  implicit def intoSchema[L](implicit allowed: AllowedLanguages[L]): IntoSchema[UnstructuredText[L]] =
    new IntoSchema[UnstructuredText[L]] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, Unstructured.textType(allowed.values))
      override def toValue(value: UnstructuredText[L]): SchemaValue =
        Unstructured.textToSchemaValue(value, allowed.values)
    }

  implicit def fromSchema[L](implicit allowed: AllowedLanguages[L]): FromSchema[UnstructuredText[L]] =
    new FromSchema[UnstructuredText[L]] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, UnstructuredText[L]] =
        Unstructured.textFromSchemaValue(value, allowed.values).map(_.asInstanceOf[UnstructuredText[L]])
    }

  def concreteCodec[L](implicit allowed: AllowedLanguages[L]): ConcreteCodec[UnstructuredText[L]] =
    new ConcreteCodec[UnstructuredText[L]] {
      def write(value: UnstructuredText[L], out: WireValues): Int = value match {
        case Url(url) =>
          Unstructured.writeCase(
            out,
            Unstructured.UrlCase,
            WitSchemaValueNode.UrlValue(UnicodeText.encodeText("url", url))
          )
        case Inline(text, languageCode) =>
          val language = languageCode.map(_.toString)
          Unstructured.checkEncode(language, allowed.values, "language code")
          Unstructured.writeCase(
            out,
            Unstructured.InlineCase,
            WitSchemaValueNode.TextValue(WitTextValuePayload(UnicodeText.encodeText("text", text), language))
          )
      }

      def read(in: WireValuesReader, index: Int): UnstructuredText[L] = {
        def inlineValue(payload: WitTextValuePayload): UnstructuredText[L] = {
          Unstructured
            .disallowed(payload.language, allowed.values, "language code")
            .foreach(message => throw SchemaDecodeError(message))
          Inline[L](payload.text, payload.language.asInstanceOf[Option[L]])
        }
        in.at(index) {
          case WitSchemaValueNode.VariantValue(WitVariantValuePayload(Unstructured.InlineCase, Some(payload))) =>
            in.at(payload) { case WitSchemaValueNode.TextValue(text) => inlineValue(text) }
          case WitSchemaValueNode.VariantValue(WitVariantValuePayload(Unstructured.UrlCase, Some(payload))) =>
            in.at(payload) { case WitSchemaValueNode.UrlValue(url) => Url[L](url) }
          case WitSchemaValueNode.TextValue(text) => inlineValue(text)
        }
      }

      def describe(out: WireTypes): Int =
        Unstructured.describeVariant(
          out,
          WitSchemaTypeBody.TextType(TextRestrictions(languages = allowed.values)),
          Role.UnstructuredText
        )
    }
}

/**
 * Unstructured binary data: either inline bytes with an optional MIME type, or
 * a URL that refers to the data.
 *
 * The type parameter `M` is the allow-list of MIME types:
 *   - `UnstructuredBinary[String]` accepts any MIME type;
 *   - `UnstructuredBinary["image/png" | "image/jpeg"]` accepts only the listed
 *     types, and the inline `mimeType` is typed accordingly.
 *
 * The schema is `variant { inline: binary, url: url }` with the
 * `unstructured-binary` role. When an agent method returns this type (also
 * inside `Option` or on the `Right` side of `Either`), the HTTP layer renders
 * the inline bytes as the raw response body with the MIME type as its content
 * type.
 */
sealed trait UnstructuredBinary[M] extends Product with Serializable

object UnstructuredBinary {
  final case class Url[M](url: String)                                      extends UnstructuredBinary[M]
  final case class Inline[M](data: Array[Byte], mimeType: Option[M] = None) extends UnstructuredBinary[M] {
    override def equals(other: Any): Boolean = other match {
      case that: Inline[?] => java.util.Arrays.equals(data, that.data) && mimeType == that.mimeType
      case _               => false
    }
    override def hashCode: Int    = 31 * java.util.Arrays.hashCode(data) + mimeType.hashCode
    override def toString: String = s"Inline(<${data.length} bytes>, $mimeType)"
  }

  def fromUrl[M](url: String): UnstructuredBinary[M]                       = Url(url)
  def fromInline[M](data: Array[Byte]): UnstructuredBinary[M]              = Inline(data, None)
  def fromInline[M](data: Array[Byte], mimeType: M): UnstructuredBinary[M] = Inline(data, Some(mimeType))

  implicit def schema[M](implicit allowed: AllowedMimeTypes[M]): Schema[UnstructuredBinary[M]] =
    Schema[DV]
      .transform[UnstructuredBinary[M]](
        dv =>
          Unstructured
            .validateBinary(Unstructured.binaryFromDynamic(dv), allowed.values)
            .asInstanceOf[UnstructuredBinary[M]],
        Unstructured.binaryToDynamic
      )
      .modifiers(Unstructured.restrictionModifier(allowed.values).toList)

  implicit def intoSchema[M](implicit allowed: AllowedMimeTypes[M]): IntoSchema[UnstructuredBinary[M]] =
    new IntoSchema[UnstructuredBinary[M]] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, Unstructured.binaryType(allowed.values))
      override def toValue(value: UnstructuredBinary[M]): SchemaValue =
        Unstructured.binaryToSchemaValue(value, allowed.values)
    }

  implicit def fromSchema[M](implicit allowed: AllowedMimeTypes[M]): FromSchema[UnstructuredBinary[M]] =
    new FromSchema[UnstructuredBinary[M]] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, UnstructuredBinary[M]] =
        Unstructured.binaryFromSchemaValue(value, allowed.values).map(_.asInstanceOf[UnstructuredBinary[M]])
    }

  def concreteCodec[M](implicit allowed: AllowedMimeTypes[M]): ConcreteCodec[UnstructuredBinary[M]] =
    new ConcreteCodec[UnstructuredBinary[M]] {
      def write(value: UnstructuredBinary[M], out: WireValues): Int = value match {
        case Url(url) =>
          Unstructured.writeCase(
            out,
            Unstructured.UrlCase,
            WitSchemaValueNode.UrlValue(UnicodeText.encodeText("url", url))
          )
        case Inline(data, mimeType) =>
          val mime = mimeType.map(_.toString)
          Unstructured.checkEncode(mime, allowed.values, "MIME type")
          Unstructured.writeCase(
            out,
            Unstructured.InlineCase,
            WitSchemaValueNode.BinaryValue(WitBinaryValuePayload(data.toVector, mime))
          )
      }

      def read(in: WireValuesReader, index: Int): UnstructuredBinary[M] = {
        def inlineValue(payload: WitBinaryValuePayload): UnstructuredBinary[M] = {
          Unstructured
            .disallowed(payload.mimeType, allowed.values, "MIME type")
            .foreach(message => throw SchemaDecodeError(message))
          Inline[M](payload.bytes.toArray, payload.mimeType.asInstanceOf[Option[M]])
        }
        in.at(index) {
          case WitSchemaValueNode.VariantValue(WitVariantValuePayload(Unstructured.InlineCase, Some(payload))) =>
            in.at(payload) { case WitSchemaValueNode.BinaryValue(binary) => inlineValue(binary) }
          case WitSchemaValueNode.VariantValue(WitVariantValuePayload(Unstructured.UrlCase, Some(payload))) =>
            in.at(payload) { case WitSchemaValueNode.UrlValue(url) => Url[M](url) }
          case WitSchemaValueNode.BinaryValue(binary) => inlineValue(binary)
        }
      }

      def describe(out: WireTypes): Int =
        Unstructured.describeVariant(
          out,
          WitSchemaTypeBody.BinaryType(BinaryRestrictions(mimeTypes = allowed.values)),
          Role.UnstructuredBinary
        )
    }
}

/**
 * Evidence that `L` is a valid language allow-list for [[UnstructuredText]]:
 * `String` (unrestricted) or a union of string literal types. Derived by the
 * compiler only.
 */
final class AllowedLanguages[L] @publicInBinary private[golem] (val values: Option[List[String]])

object AllowedLanguages {
  inline given derived[L]: AllowedLanguages[L] = ${ AllowedStringsMacro.languages[L] }
}

/**
 * Evidence that `M` is a valid MIME type allow-list for [[UnstructuredBinary]]:
 * `String` (unrestricted) or a union of string literal types. Derived by the
 * compiler only.
 */
final class AllowedMimeTypes[M] @publicInBinary private[golem] (val values: Option[List[String]])

object AllowedMimeTypes {
  inline given derived[M]: AllowedMimeTypes[M] = ${ AllowedStringsMacro.mimeTypes[M] }
}

/** Shared schema, value and dynamic-value mapping of the unstructured types. */
private[golem] object Unstructured {
  final val InlineCase     = 0
  final val UrlCase        = 1
  final val InlineCaseName = "inline"
  final val UrlCaseName    = "url"

  final val TextFqn   = "golem.schema.UnstructuredText"
  final val BinaryFqn = "golem.schema.UnstructuredBinary"

  private final val RestrictionKey = "golem.schema.unstructured.allowed"
  private final val Separator      = "\u001f"

  def textType(languages: Option[List[String]]): SchemaType =
    variantType(SchemaType(SchemaTypeBody.TextType(TextRestrictions(languages = languages))), Role.UnstructuredText)

  def binaryType(mimeTypes: Option[List[String]]): SchemaType =
    variantType(
      SchemaType(SchemaTypeBody.BinaryType(BinaryRestrictions(mimeTypes = mimeTypes))),
      Role.UnstructuredBinary
    )

  private def variantType(inlineType: SchemaType, role: Role): SchemaType =
    SchemaType(
      SchemaTypeBody.VariantType(
        List(
          VariantCaseType(InlineCaseName, Some(inlineType)),
          VariantCaseType(UrlCaseName, Some(SchemaType(SchemaTypeBody.UrlType(UrlRestrictions.empty))))
        )
      ),
      MetadataEnvelope(role = Some(role))
    )

  def describeVariant(out: WireTypes, inlineBody: WitSchemaTypeBody, role: Role): Int = {
    val inlineType = out.add(inlineBody)
    val url        = out.add(WitSchemaTypeBody.UrlType(UrlRestrictions.empty))
    out.add(
      WitSchemaTypeBody.VariantType(
        Vector(
          WitVariantCaseType(InlineCaseName, Some(inlineType), MetadataEnvelope.empty),
          WitVariantCaseType(UrlCaseName, Some(url), MetadataEnvelope.empty)
        )
      ),
      MetadataEnvelope(role = Some(role))
    )
  }

  def writeCase(out: WireValues, caseIndex: Int, payload: WitSchemaValueNode): Int = {
    val child = out.add(payload)
    out.add(WitSchemaValueNode.VariantValue(WitVariantValuePayload(caseIndex, Some(child))))
  }

  // ---------------------------------------------------------------------------
  // Restrictions
  // ---------------------------------------------------------------------------

  def restrictionModifier(values: Option[List[String]]): Option[Modifier.Reflect] =
    values.map(vs => Modifier.config(RestrictionKey, vs.mkString(Separator)))

  def restrictionFromModifiers(modifiers: Seq[Modifier.Reflect]): Option[List[String]] =
    modifiers.collectFirst { case Modifier.config(RestrictionKey, value) => value.split(Separator, -1).toList }

  def checkEncode(value: Option[String], allowed: Option[List[String]], what: String): Unit = {
    value.foreach(UnicodeText.encodeText(what, _))
    disallowed(value, allowed, what).foreach(message => throw SchemaEncodeError(message))
  }

  def disallowed(value: Option[String], allowed: Option[List[String]], what: String): Option[String] =
    (value, allowed) match {
      case (Some(v), Some(vs)) if !vs.contains(v) =>
        Some(s"$what '$v' is not allowed; allowed: ${vs.mkString(", ")}")
      case _ => None
    }

  /**
   * A missing language code is always accepted; a present one must be in a
   * non-empty allow-list.
   */
  def validateText(value: UnstructuredText[?], allowed: Option[List[String]]): UnstructuredText[?] =
    value match {
      case UnstructuredText.Inline(_, language) =>
        disallowed(language.map(_.toString), allowed, "language code").foreach(m => throw FromSchemaError(m))
        value
      case _ => value
    }

  /**
   * A missing MIME type is always accepted; a present one must be in a
   * non-empty allow-list.
   */
  def validateBinary(value: UnstructuredBinary[?], allowed: Option[List[String]]): UnstructuredBinary[?] =
    value match {
      case UnstructuredBinary.Inline(_, mimeType) =>
        disallowed(mimeType.map(_.toString), allowed, "MIME type").foreach(m => throw FromSchemaError(m))
        value
      case _ => value
    }

  // ---------------------------------------------------------------------------
  // Scala value <-> SchemaValue
  // ---------------------------------------------------------------------------

  def textToSchemaValue(value: UnstructuredText[?], allowed: Option[List[String]]): SchemaValue =
    value match {
      case UnstructuredText.Url(url) =>
        SchemaValue.VariantValue(UrlCase, Some(SchemaValue.UrlValue(UnicodeText.encodeText("url", url))))
      case UnstructuredText.Inline(text, languageCode) =>
        val language = languageCode.map(_.toString)
        checkEncode(language, allowed, "language code")
        SchemaValue.VariantValue(
          InlineCase,
          Some(SchemaValue.TextValue(UnicodeText.encodeText("text", text), language))
        )
    }

  /** Accepts the `variant { inline, url }` value or a bare `text` value. */
  def textFromSchemaValue(
    value: SchemaValue,
    allowed: Option[List[String]]
  ): Either[FromSchemaError, UnstructuredText[String]] = {
    def inlineValue(text: String, language: Option[String]) =
      disallowed(language, allowed, "language code") match {
        case Some(message) => Left(FromSchemaError(message))
        case None          => Right(UnstructuredText.Inline[String](text, language))
      }
    value match {
      case SchemaValue.VariantValue(InlineCase, Some(SchemaValue.TextValue(text, language))) =>
        inlineValue(text, language)
      case SchemaValue.VariantValue(UrlCase, Some(SchemaValue.UrlValue(url))) =>
        Right(UnstructuredText.Url[String](url))
      case SchemaValue.TextValue(text, language) => inlineValue(text, language)
      case other                                 =>
        Left(FromSchemaError(s"expected unstructured-text variant or text value, got $other"))
    }
  }

  def binaryToSchemaValue(value: UnstructuredBinary[?], allowed: Option[List[String]]): SchemaValue =
    value match {
      case UnstructuredBinary.Url(url) =>
        SchemaValue.VariantValue(UrlCase, Some(SchemaValue.UrlValue(UnicodeText.encodeText("url", url))))
      case UnstructuredBinary.Inline(data, mimeType) =>
        val mime = mimeType.map(_.toString)
        checkEncode(mime, allowed, "MIME type")
        SchemaValue.VariantValue(InlineCase, Some(SchemaValue.BinaryValue(data.toVector, mime)))
    }

  /** Accepts the `variant { inline, url }` value or a bare `binary` value. */
  def binaryFromSchemaValue(
    value: SchemaValue,
    allowed: Option[List[String]]
  ): Either[FromSchemaError, UnstructuredBinary[String]] = {
    def inlineValue(bytes: Vector[Byte], mimeType: Option[String]) =
      disallowed(mimeType, allowed, "MIME type") match {
        case Some(message) => Left(FromSchemaError(message))
        case None          => Right(UnstructuredBinary.Inline[String](bytes.toArray, mimeType))
      }
    value match {
      case SchemaValue.VariantValue(InlineCase, Some(SchemaValue.BinaryValue(bytes, mimeType))) =>
        inlineValue(bytes, mimeType)
      case SchemaValue.VariantValue(UrlCase, Some(SchemaValue.UrlValue(url))) =>
        Right(UnstructuredBinary.Url[String](url))
      case SchemaValue.BinaryValue(bytes, mimeType) => inlineValue(bytes, mimeType)
      case other                                    =>
        Left(FromSchemaError(s"expected unstructured-binary variant or binary value, got $other"))
    }
  }

  // ---------------------------------------------------------------------------
  // Scala value <-> zio-blocks DynamicValue (used by structural derivation when
  // the types are nested in Option, Either, records or collections)
  // ---------------------------------------------------------------------------

  private def str(value: String): DV = DV.Primitive(PrimitiveValue.String(value))

  private def optionalField(name: String, value: Option[String]): Chunk[(String, DV)] =
    value.fold(Chunk.empty[(String, DV)])(v => Chunk(name -> str(v)))

  private def stringField(fields: Chunk[(String, DV)], name: String): Option[String] =
    fields.collectFirst { case (`name`, DV.Primitive(PrimitiveValue.String(v))) => v }

  def textToDynamic(value: UnstructuredText[?]): DV = value match {
    case UnstructuredText.Url(url)                   => DV.Variant(UrlCaseName, str(url))
    case UnstructuredText.Inline(text, languageCode) =>
      DV.Variant(
        InlineCaseName,
        DV.Record(Chunk("text" -> str(text)) ++ optionalField("language", languageCode.map(_.toString)))
      )
  }

  def textFromDynamic(value: DV): UnstructuredText[String] = value match {
    case DV.Variant(UrlCaseName, DV.Primitive(PrimitiveValue.String(url))) => UnstructuredText.Url(url)
    case DV.Variant(InlineCaseName, DV.Record(fields))                     =>
      UnstructuredText.Inline(
        stringField(fields, "text").getOrElse(throw FromSchemaError("unstructured text is missing its text")),
        stringField(fields, "language")
      )
    case other => throw FromSchemaError(s"unexpected dynamic value for UnstructuredText: $other")
  }

  def binaryToDynamic(value: UnstructuredBinary[?]): DV = value match {
    case UnstructuredBinary.Url(url)               => DV.Variant(UrlCaseName, str(url))
    case UnstructuredBinary.Inline(data, mimeType) =>
      DV.Variant(
        InlineCaseName,
        DV.Record(
          Chunk("data" -> DV.Sequence(Chunk.fromArray(data).map[DV](b => DV.Primitive(PrimitiveValue.Byte(b))))) ++
            optionalField("mimeType", mimeType.map(_.toString))
        )
      )
  }

  def binaryFromDynamic(value: DV): UnstructuredBinary[String] = value match {
    case DV.Variant(UrlCaseName, DV.Primitive(PrimitiveValue.String(url))) => UnstructuredBinary.Url(url)
    case DV.Variant(InlineCaseName, DV.Record(fields))                     =>
      val data = fields.collectFirst { case ("data", DV.Sequence(bytes)) =>
        bytes.map {
          case DV.Primitive(PrimitiveValue.Byte(b)) => b
          case other                                => throw FromSchemaError(s"unexpected byte in UnstructuredBinary: $other")
        }.toArray
      }
      UnstructuredBinary.Inline(
        data.getOrElse(throw FromSchemaError("unstructured binary is missing its data")),
        stringField(fields, "mimeType")
      )
    case other => throw FromSchemaError(s"unexpected dynamic value for UnstructuredBinary: $other")
  }
}
