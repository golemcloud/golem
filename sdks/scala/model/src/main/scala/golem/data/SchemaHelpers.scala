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
 * Utility functions for working with structured schemas and values.
 *
 * These helpers simplify common operations like extracting single-element
 * schemas/values from their wrappers.
 */
object SchemaHelpers {

  /**
   * Extracts the single element schema from a structured schema.
   *
   * Only succeeds for tuple schemas with exactly one element.
   *
   * @param structured
   *   The structured schema to unwrap
   * @return
   *   The inner element schema, or an error message
   */
  def singleElementSchema(structured: StructuredSchema): Either[String, ElementSchema] =
    structured match {
      case StructuredSchema.Tuple(elements) =>
        elements match {
          case head :: Nil => Right(head.schema)
          case Nil         => Left("Structured schema has no elements")
          case _           => Left("Structured schema has multiple elements")
        }
      case StructuredSchema.Multimodal(_) =>
        Left("Nested multimodal schemas are not supported")
    }

  /**
   * Extracts the single element value from a structured value.
   *
   * Only succeeds for tuple values with exactly one element.
   *
   * @param structured
   *   The structured value to unwrap
   * @return
   *   The inner element value, or an error message
   */
  def singleElementValue(structured: StructuredValue): Either[String, ElementValue] =
    structured match {
      case StructuredValue.Tuple(elements) =>
        elements match {
          case head :: Nil => Right(head.value)
          case Nil         => Left("Structured value has no elements")
          case _           => Left("Structured value has multiple elements")
        }
      case StructuredValue.Multimodal(_) =>
        Left("Nested multimodal values are not supported")
    }

  /**
   * Wraps an element value in a single-element structured value.
   *
   * @param value
   *   The element value to wrap
   * @return
   *   A structured value containing the element
   */
  def wrapElementValue(value: ElementValue): StructuredValue =
    StructuredValue.single(value)
}
