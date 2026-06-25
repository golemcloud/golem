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

package golem.bridge.runtime

import golem.bridge.runtime.SchemaValue._

/**
 * Ergonomic wrapper for the role-marked unstructured-text schema type
 * (`variant { inline: text<restrictions>, url: url }`), mirroring the Golem
 * TypeScript bridge's `UnstructuredText`. A value is either inline text (with an
 * optional language code) or a URL reference.
 */
sealed trait UnstructuredText extends Product with Serializable
object UnstructuredText {
  final case class Inline(value: String, languageCode: Option[String]) extends UnstructuredText
  final case class Url(value: String)                                  extends UnstructuredText

  /** Inline unstructured text with an optional BCP-47 language code. */
  def fromInline(value: String, languageCode: Option[String] = None): UnstructuredText =
    Inline(value, languageCode)

  /** A URL reference to unstructured text. */
  def fromUrl(url: String): UnstructuredText = Url(url)

  // Variant case indices of the canonical role-marked unstructured wrapper:
  // `variant { inline: text, url: url }`.
  private val InlineCase = 0
  private val UrlCase    = 1

  /** Encodes into the schema-native `variant { inline, url }` value. */
  def toSchemaValue(input: UnstructuredText): SchemaValue = input match {
    case Inline(value, languageCode) =>
      VariantValue(InlineCase, Some(TextValue(value, languageCode)))
    case Url(url) =>
      VariantValue(UrlCase, Some(UrlValue(url)))
  }

  /**
   * Decodes a schema-native unstructured-text `variant { inline, url }` value,
   * validating the language tag against `allowedCodes` when the agent declares
   * a fixed set. A missing language is always allowed (lenient decode, matching
   * the server's `check_text`).
   */
  def fromSchemaValue(
    parameterName: String,
    value: SchemaValue,
    allowedCodes: List[String]
  ): Either[String, UnstructuredText] = value match {
    case VariantValue(UrlCase, Some(UrlValue(url))) =>
      Right(Url(url))
    case VariantValue(InlineCase, Some(TextValue(text, language))) =>
      if (allowedCodes.nonEmpty && language.exists(l => !allowedCodes.contains(l)))
        Left(
          s"Invalid value for parameter $parameterName. Language code `${language.get}` is not allowed. " +
            s"Allowed codes: ${allowedCodes.mkString(", ")}"
        )
      else
        Right(Inline(text, language))
    case other =>
      Left(s"Invalid value for parameter $parameterName. Expected an unstructured-text variant, got $other")
  }
}

/**
 * Ergonomic wrapper for the role-marked unstructured-binary schema type
 * (`variant { inline: binary<restrictions>, url: url }`), mirroring the Golem
 * TypeScript bridge's `UnstructuredBinary`. A value is either inline bytes (with
 * an optional MIME type) or a URL reference.
 */
sealed trait UnstructuredBinary extends Product with Serializable
object UnstructuredBinary {
  final case class Inline(bytes: Vector[Byte], mimeType: Option[String]) extends UnstructuredBinary
  final case class Url(value: String)                                    extends UnstructuredBinary

  /** Inline unstructured bytes with an optional MIME type. */
  def fromInline(bytes: Vector[Byte], mimeType: Option[String] = None): UnstructuredBinary =
    Inline(bytes, mimeType)

  /** A URL reference to unstructured binary content. */
  def fromUrl(url: String): UnstructuredBinary = Url(url)

  private val InlineCase = 0
  private val UrlCase    = 1

  /** Encodes into the schema-native `variant { inline, url }` value. */
  def toSchemaValue(input: UnstructuredBinary): SchemaValue = input match {
    case Inline(bytes, mimeType) =>
      VariantValue(InlineCase, Some(BinaryValue(bytes, mimeType)))
    case Url(url) =>
      VariantValue(UrlCase, Some(UrlValue(url)))
  }

  /** Decodes a schema-native unstructured-binary `variant { inline, url }` value. */
  def fromSchemaValue(
    parameterName: String,
    value: SchemaValue
  ): Either[String, UnstructuredBinary] = value match {
    case VariantValue(UrlCase, Some(UrlValue(url))) =>
      Right(Url(url))
    case VariantValue(InlineCase, Some(BinaryValue(bytes, mimeType))) =>
      Right(Inline(bytes, mimeType))
    case other =>
      Left(s"Invalid value for parameter $parameterName. Expected an unstructured-binary variant, got $other")
  }
}
