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

package golem.data.multimodal

import golem.data._

/**
 * Wrapper that lifts a case class into a multimodal schema.
 *
 * Multimodal payloads combine different content types (text, binary,
 * component-model) in a single structure. Use this wrapper when your data
 * contains mixed modalities.
 *
 * @tparam A
 *   The underlying case class type
 * @param value
 *   The wrapped value
 */
final case class Multimodal[A](value: A)

/**
 * Companion providing [[GolemSchema]] derivation for [[Multimodal]] types.
 */
object Multimodal {

  /**
   * Derives a multimodal GolemSchema from the underlying type's schema.
   *
   * The resulting schema tags the structure as multimodal rather than tuple,
   * enabling the host to handle mixed-modality content appropriately.
   *
   * @tparam A
   *   The underlying type (must have a GolemSchema instance)
   * @param base
   *   The underlying GolemSchema
   * @return
   *   A GolemSchema for Multimodal[A]
   */
  implicit def derived[A](implicit base: GolemSchema[A]): GolemSchema[Multimodal[A]] =
    new GolemSchema[Multimodal[A]] {
      private val modalitySchema: List[NamedElementSchema] =
        schemaAsModality(base.schema)

      override val schema: StructuredSchema =
        StructuredSchema.Multimodal(modalitySchema)

      override def encode(value: Multimodal[A]): Either[String, StructuredValue] =
        base.encode(value.value).flatMap(valueAsModality).map(elements => StructuredValue.Multimodal(elements))

      override def decode(structured: StructuredValue): Either[String, Multimodal[A]] =
        structured match {
          case StructuredValue.Multimodal(elements) =>
            base.decode(StructuredValue.Tuple(elements)).map(Multimodal(_))
          case other =>
            Left(s"Expected multimodal structured value, found $other")
        }

      override def elementSchema: ElementSchema =
        base.elementSchema

      override def encodeElement(value: Multimodal[A]): Either[String, ElementValue] =
        base.encodeElement(value.value)

      override def decodeElement(value: ElementValue): Either[String, Multimodal[A]] =
        base.decodeElement(value).map(Multimodal(_))
    }

  private def schemaAsModality(structured: StructuredSchema): List[NamedElementSchema] =
    structured match {
      case StructuredSchema.Tuple(elements)      => elements
      case StructuredSchema.Multimodal(elements) => elements
    }

  private def valueAsModality(value: StructuredValue): Either[String, List[NamedElementValue]] =
    value match {
      case StructuredValue.Tuple(elements)      => Right(elements)
      case StructuredValue.Multimodal(elements) => Right(elements)
    }
}
