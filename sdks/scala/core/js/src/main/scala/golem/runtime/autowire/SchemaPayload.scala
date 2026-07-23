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

package golem.runtime.autowire

import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsSchemaGraph, JsSchemaValueTree}
import golem.schema.{FromSchema, FromSchemaError, IntoSchema, SchemaGraph}
import golem.schema.wire.SchemaWire

/**
 * The `golem:core/types@2.0.0` host-payload bridge: the single hub that turns
 * the schema-native typeclasses ([[IntoSchema]] / [[FromSchema]], Slice 2) into
 * the JS facades the host boundary speaks ([[JsSchemaGraph]] /
 * [[JsSchemaValueTree]], Slice 3).
 *
 * The pipeline reuses the already-tested layers end to end:
 * `IntoSchema/FromSchema` (recursive model) <-> `SchemaWire` (Slice 1 recursive
 * <-> flat `Wit*`) <-> `SchemaWireInterop` (Slice 3 flat `Wit*` <-> JS `Js*`).
 *
 * This is the host payload encoder for the `golem:core/types@2.0.0` schema
 * model.
 */
object SchemaPayload {

  /** The self-contained JS schema graph for `A`. */
  def graph[A](implicit ev: IntoSchema[A]): JsSchemaGraph =
    graphFromModel(ev.graph)

  /** Convert an already-built recursive [[SchemaGraph]] to its JS facade. */
  def graphFromModel(g: SchemaGraph): JsSchemaGraph =
    SchemaWireInterop.graphToJs(SchemaWire.schemaGraphToWit(g))

  /** Encode a value of `A` into a JS schema value tree. */
  def encode[A](value: A)(implicit ev: IntoSchema[A]): JsSchemaValueTree =
    SchemaWireInterop.valueTreeToJs(SchemaWire.schemaValueToWit(ev.toValue(value)))

  /** Decode a JS schema value tree into a value of `A`. */
  def decode[A](tree: JsSchemaValueTree)(implicit ev: FromSchema[A]): Either[FromSchemaError, A] =
    ev.fromValue(SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(tree)))
}
