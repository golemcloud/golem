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

import scala.collection.immutable.ListMap

/** A named type definition inside a [[SchemaGraph]]. */
final case class SchemaTypeDef(body: SchemaType, name: Option[String] = None)

/**
 * A self-contained schema graph: a registry of named definitions (keyed by
 * stable `type-id`) plus a root type. `SchemaTypeBody.RefType` bodies reference
 * entries in `defs`. Anywhere a schema travels with a value the payload owns
 * its own graph — there is no implicit external registry consumers must look
 * up.
 *
 * `defs` is a [[ListMap]] so iteration order is deterministic; the WIT codecs
 * additionally sort by id when flattening.
 */
final case class SchemaGraph(defs: ListMap[String, SchemaTypeDef], root: SchemaType)

/** A typed value: a self-contained schema graph paired with a value tree. */
final case class TypedSchemaValue(graph: SchemaGraph, value: SchemaValue)
