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

package golem.data.unstructured

import golem.data.SchemaHelpers.singleElementValue
import golem.data._

/**
 * Type class defining allowed language codes for text segments.
 *
 * Implement this for your language enum to constrain [[TextSegment]] content.
 *
 * @tparam A
 *   The language constraint enum type
 */
trait AllowedLanguages[A] {

  /** The allowed language codes, or None if any language is permitted. */
  def codes: Option[List[String]]
}

object AllowedLanguages {

  /** Marker type for unconstrained languages. */
  sealed trait Any

  /** Instance that permits any language. */
  implicit val anyLanguage: AllowedLanguages[Any] = new AllowedLanguages[Any] {
    override val codes: Option[List[String]] = None
  }
}

/**
 * Type class defining allowed MIME types for binary segments.
 *
 * Implement this for your MIME enum to constrain [[BinarySegment]] content.
 *
 * @tparam A
 *   The MIME constraint enum type
 */
trait AllowedMimeTypes[A] {

  /** The allowed MIME types, or None if any type is permitted. */
  def mimeTypes: Option[List[String]]
}

object AllowedMimeTypes {

  /** Marker type for unconstrained MIME types. */
  sealed trait Any

  /** Instance that permits any MIME type. */
  implicit val anyMimeType: AllowedMimeTypes[Any] = new AllowedMimeTypes[Any] {
    override val mimeTypes: Option[List[String]] = None
  }
}

/**
 * A text segment with compile-time language constraints.
 *
 * Use this for text content in multimodal payloads where language restrictions
 * should be enforced at the schema level.
 *
 * @tparam Lang
 *   The language constraint type (must have [[AllowedLanguages]] instance)
 * @param value
 *   The underlying text value
 */
final case class TextSegment[Lang](value: UnstructuredTextValue)

object TextSegment {

  /**
   * Creates an inline text segment.
   *
   * @param text
   *   The text content
   * @param languageCode
   *   Optional ISO language code (e.g., "en")
   * @return
   *   A text segment with inline content
   */
  def inline[Lang](text: String, languageCode: Option[String] = None): TextSegment[Lang] =
    TextSegment(UnstructuredTextValue.Inline(text, languageCode))

  /**
   * Creates a URL-referenced text segment.
   *
   * @param value
   *   The URL pointing to the text content
   * @return
   *   A text segment referencing remote content
   */
  def url[Lang](value: String): TextSegment[Lang] =
    TextSegment(UnstructuredTextValue.Url(value))

  /**
   * Derives a GolemSchema for TextSegment with the given language constraints.
   */
  implicit def textSegmentSchema[Lang](implicit allowed: AllowedLanguages[Lang]): GolemSchema[TextSegment[Lang]] =
    new GolemSchema[TextSegment[Lang]] {
      override val schema: StructuredSchema =
        StructuredSchema.single(ElementSchema.UnstructuredText(allowed.codes))

      override def encode(value: TextSegment[Lang]): Either[String, StructuredValue] =
        Right(StructuredValue.single(ElementValue.UnstructuredText(value.value)))

      override def decode(value: StructuredValue): Either[String, TextSegment[Lang]] =
        singleElementValue(value).flatMap {
          case ElementValue.UnstructuredText(textValue) =>
            Right(TextSegment(textValue))
          case other =>
            Left(s"Expected unstructured-text element, found $other")
        }

      override def elementSchema: ElementSchema =
        ElementSchema.UnstructuredText(allowed.codes)

      override def encodeElement(value: TextSegment[Lang]): Either[String, ElementValue] =
        Right(ElementValue.UnstructuredText(value.value))

      override def decodeElement(value: ElementValue): Either[String, TextSegment[Lang]] =
        value match {
          case ElementValue.UnstructuredText(v) => Right(TextSegment(v))
          case other                            => Left(s"Expected unstructured-text element, found: ${other.getClass.getSimpleName}")
        }
    }
}

/**
 * A binary segment with compile-time MIME type constraints.
 *
 * Use this for binary content in multimodal payloads where MIME type
 * restrictions should be enforced at the schema level.
 *
 * @tparam Descriptor
 *   The MIME constraint type (must have [[AllowedMimeTypes]] instance)
 * @param value
 *   The underlying binary value
 */
final case class BinarySegment[Descriptor](value: UnstructuredBinaryValue)

object BinarySegment {

  /**
   * Creates an inline binary segment.
   *
   * @param bytes
   *   The binary data
   * @param mimeType
   *   The content's MIME type
   * @return
   *   A binary segment with inline content
   */
  def inline[Descriptor](bytes: Array[Byte], mimeType: String): BinarySegment[Descriptor] =
    BinarySegment(UnstructuredBinaryValue.Inline(bytes, mimeType))

  /**
   * Creates a URL-referenced binary segment.
   *
   * @param value
   *   The URL pointing to the binary content
   * @return
   *   A binary segment referencing remote content
   */
  def url[Descriptor](value: String): BinarySegment[Descriptor] =
    BinarySegment(UnstructuredBinaryValue.Url(value))

  /**
   * Derives a GolemSchema for BinarySegment with the given MIME constraints.
   */
  implicit def binarySegmentSchema[Descriptor](implicit
    allowed: AllowedMimeTypes[Descriptor]
  ): GolemSchema[BinarySegment[Descriptor]] =
    new GolemSchema[BinarySegment[Descriptor]] {
      override val schema: StructuredSchema =
        StructuredSchema.single(ElementSchema.UnstructuredBinary(allowed.mimeTypes))

      override def encode(value: BinarySegment[Descriptor]): Either[String, StructuredValue] =
        Right(StructuredValue.single(ElementValue.UnstructuredBinary(value.value)))

      override def decode(value: StructuredValue): Either[String, BinarySegment[Descriptor]] =
        singleElementValue(value).flatMap {
          case ElementValue.UnstructuredBinary(binaryValue) =>
            Right(BinarySegment(binaryValue))
          case other =>
            Left(s"Expected unstructured-binary element, found $other")
        }

      override def elementSchema: ElementSchema =
        ElementSchema.UnstructuredBinary(allowed.mimeTypes)

      override def encodeElement(value: BinarySegment[Descriptor]): Either[String, ElementValue] =
        Right(ElementValue.UnstructuredBinary(value.value))

      override def decodeElement(value: ElementValue): Either[String, BinarySegment[Descriptor]] =
        value match {
          case ElementValue.UnstructuredBinary(v) => Right(BinarySegment(v))
          case other                              => Left(s"Expected unstructured-binary element, found: ${other.getClass.getSimpleName}")
        }
    }
}
