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
 * Encodes a Scala type `A` into the `golem:core/types@2.0.0` schema model: its
 * self-contained [[SchemaGraph]] (memoized per instance) plus a value encoder
 * producing a structural [[SchemaValue]].
 *
 * Mirrors the Rust SDK's `IntoSchema`. The graph is derived structurally from
 * the zio-blocks `Schema[A]` by [[Derivation]]; rich types / annotations are
 * intentionally out of scope here (see `scala-sdk.md`, Slice 2).
 */
trait IntoSchema[A] {

  /** The self-contained schema graph for `A` (computed once per instance). */
  def graph: SchemaGraph

  /** Encode a value of `A` into its structural value tree. */
  def toValue(value: A): SchemaValue

  /** Encode a value of `A` into a self-contained [[TypedSchemaValue]]. */
  final def toTyped(value: A): TypedSchemaValue = TypedSchemaValue(graph, toValue(value))
}

object IntoSchema {

  def apply[A](implicit ev: IntoSchema[A]): IntoSchema[A] = ev

  implicit def derived[A](implicit schema: Schema[A]): IntoSchema[A] =
    new IntoSchema[A] {
      override lazy val graph: SchemaGraph        = Derivation.graphOf(schema)
      override def toValue(value: A): SchemaValue = Derivation.toValue(schema, value)
    }
}
