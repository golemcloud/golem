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

package golem.data

/**
 * Describes the type of a single element within a structured schema.
 *
 * Elements come in three flavors:
 *   - [[ElementSchema.Component]] - WIT component-model types (records, enums,
 *     primitives)
 *   - [[ElementSchema.UnstructuredText]] - Text with optional language
 *     constraints
 *   - [[ElementSchema.UnstructuredBinary]] - Binary with optional MIME type
 *     constraints
 *
 * @see
 *   [[StructuredSchema]] for how elements combine into structures
 * @see
 *   [[ElementValue]] for the runtime values
 */
sealed trait ElementSchema extends Product with Serializable

object ElementSchema {

  /**
   * A WIT component-model type (the standard case for most data).
   *
   * @param dataType
   *   The underlying [[DataType]] description
   */
  final case class Component(dataType: DataType) extends ElementSchema

  /**
   * Unstructured text content with optional language restrictions.
   *
   * @param allowedLanguages
   *   If Some, the allowed language codes (e.g., ["en", "es"]). If None, any
   *   language is permitted.
   */
  final case class UnstructuredText(allowedLanguages: Option[List[String]]) extends ElementSchema

  /**
   * Unstructured binary content with optional MIME type restrictions.
   *
   * @param allowedMimeTypes
   *   If Some, the allowed MIME types (e.g., ["image/png", "image/jpeg"]). If
   *   None, any MIME type is permitted.
   */
  final case class UnstructuredBinary(allowedMimeTypes: Option[List[String]]) extends ElementSchema
}

/**
 * Represents an actual element value conforming to an [[ElementSchema]].
 *
 * @see
 *   [[ElementSchema]] for the type-level description
 */
sealed trait ElementValue extends Product with Serializable

object ElementValue {

  /**
   * A component-model value.
   *
   * @param value
   *   The underlying [[DataValue]]
   */
  final case class Component(value: DataValue) extends ElementValue

  /**
   * An unstructured text value.
   *
   * @param value
   *   The text content (inline or URL reference)
   */
  final case class UnstructuredText(value: UnstructuredTextValue) extends ElementValue

  /**
   * An unstructured binary value.
   *
   * @param value
   *   The binary content (inline or URL reference)
   */
  final case class UnstructuredBinary(value: UnstructuredBinaryValue) extends ElementValue
}

/**
 * Represents text content that may be inline or referenced by URL.
 */
sealed trait UnstructuredTextValue extends Product with Serializable

object UnstructuredTextValue {

  /**
   * Inline text content with optional language code.
   *
   * @param data
   *   The text content
   * @param languageCode
   *   Optional ISO language code (e.g., "en", "es")
   */
  final case class Inline(data: String, languageCode: Option[String]) extends UnstructuredTextValue

  /**
   * Text referenced by URL.
   *
   * @param value
   *   The URL pointing to the text content
   */
  final case class Url(value: String) extends UnstructuredTextValue
}

/**
 * Represents binary content that may be inline or referenced by URL.
 */
sealed trait UnstructuredBinaryValue extends Product with Serializable

object UnstructuredBinaryValue {

  /**
   * Inline binary content with MIME type.
   *
   * @param data
   *   The binary data
   * @param mimeType
   *   The content's MIME type (e.g., "image/png")
   */
  final case class Inline(data: Array[Byte], mimeType: String) extends UnstructuredBinaryValue

  /**
   * Binary content referenced by URL.
   *
   * @param value
   *   The URL pointing to the binary content
   */
  final case class Url(value: String) extends UnstructuredBinaryValue
}
