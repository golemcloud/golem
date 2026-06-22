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

import zio.blocks.schema.Schema

/**
 * Decodes a structural [[SchemaValue]] back into a Scala value of type `A`.
 *
 * Mirrors the Rust SDK's `FromSchema`. The decoding is structurally driven by
 * the zio-blocks `Schema[A]` (via [[Derivation]]), so the value tree must match
 * the shape produced by the corresponding [[IntoSchema]].
 */
trait FromSchema[A] {

  /** Decode a structural value tree into `A`, or describe why it failed. */
  def fromValue(value: SchemaValue): Either[FromSchemaError, A]
}

object FromSchema {

  def apply[A](implicit ev: FromSchema[A]): FromSchema[A] = ev

  implicit def derived[A](implicit schema: Schema[A]): FromSchema[A] =
    new FromSchema[A] {
      override def fromValue(value: SchemaValue): Either[FromSchemaError, A] =
        Derivation.fromValue(schema, value)
    }
}
