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
 * A named element within a structured schema.
 *
 * @param name
 *   The field/element name
 * @param schema
 *   The element's type schema
 */
final case class NamedElementSchema(name: String, schema: ElementSchema)

/**
 * Represents the structure of a complex value in the Golem type system.
 *
 * Structured schemas describe how payloads are organized:
 *   - [[StructuredSchema.Tuple]] - Ordered, named fields (like a case class)
 *   - [[StructuredSchema.Multimodal]] - Mixed-modality content (text + binary)
 */
sealed trait StructuredSchema extends Product with Serializable

object StructuredSchema {

  /** Default field name for single-element schemas. */
  val DefaultFieldName: String = "value"

  /**
   * Creates a single-element tuple schema.
   *
   * This is the common case for simple types that encode to one value.
   *
   * @param element
   *   The element's type schema
   * @param name
   *   The field name (defaults to "value")
   * @return
   *   A tuple schema with one element
   */
  def single(element: ElementSchema, name: String = DefaultFieldName): StructuredSchema =
    Tuple(List(NamedElementSchema(name, element)))

  /**
   * A tuple schema - ordered, named elements forming a record-like structure.
   *
   * This is the default schema type for case classes and standard data types.
   *
   * @param elements
   *   The named elements in order
   */
  final case class Tuple(elements: List[NamedElementSchema]) extends StructuredSchema

  /**
   * A multimodal schema - elements that may mix different content types.
   *
   * Used for payloads combining text, binary, and component-model data.
   *
   * @param elements
   *   The named elements in order
   */
  final case class Multimodal(elements: List[NamedElementSchema]) extends StructuredSchema
}

/**
 * A named element within a structured value.
 *
 * @param name
 *   The field/element name (must match the schema)
 * @param value
 *   The element's actual value
 */
final case class NamedElementValue(name: String, value: ElementValue)

/**
 * Represents an actual value conforming to a [[StructuredSchema]].
 *
 * Structured values are the runtime counterpart to structured schemas:
 *   - [[StructuredValue.Tuple]] - Values for tuple schemas
 *   - [[StructuredValue.Multimodal]] - Values for multimodal schemas
 *
 * @see
 *   [[StructuredSchema]] for the type-level description
 */
sealed trait StructuredValue extends Product with Serializable

object StructuredValue {

  /**
   * Creates a single-element tuple value.
   *
   * @param value
   *   The element value
   * @param name
   *   The field name (defaults to "value")
   * @return
   *   A tuple value with one element
   */
  def single(value: ElementValue, name: String = StructuredSchema.DefaultFieldName): StructuredValue =
    Tuple(List(NamedElementValue(name, value)))

  /**
   * A tuple value containing named element values.
   *
   * @param elements
   *   The element values in schema order
   */
  final case class Tuple(elements: List[NamedElementValue]) extends StructuredValue

  /**
   * A multimodal value containing named element values.
   *
   * @param elements
   *   The element values in schema order
   */
  final case class Multimodal(elements: List[NamedElementValue]) extends StructuredValue
}
