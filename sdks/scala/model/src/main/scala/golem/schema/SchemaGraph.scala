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
final case class SchemaGraph(defs: ListMap[String, SchemaTypeDef], root: SchemaType) {
  def containsStream: Boolean = {
    import SchemaTypeBody._

    def visit(schemaType: SchemaType, visiting: Set[String]): Boolean =
      schemaType.body match {
        case StreamType(_)                => true
        case RefType(id) if !visiting(id) =>
          defs.get(id).exists(definition => visit(definition.body, visiting + id))
        case RecordType(fields)        => fields.exists(field => visit(field.body, visiting))
        case VariantType(cases)        => cases.exists(_.payload.exists(visit(_, visiting)))
        case TupleType(elements)       => elements.exists(visit(_, visiting))
        case ListType(element)         => visit(element, visiting)
        case FixedListType(element, _) => visit(element, visiting)
        case MapType(key, value)       => visit(key, visiting) || visit(value, visiting)
        case OptionType(element)       => visit(element, visiting)
        case ResultType(ok, err)       => ok.exists(visit(_, visiting)) || err.exists(visit(_, visiting))
        case UnionType(branches)       => branches.exists(branch => visit(branch.body, visiting))
        case SecretType(spec)          => visit(spec.inner, visiting)
        case FutureType(element)       => element.exists(visit(_, visiting))
        case _                         => false
      }

    visit(root, Set.empty)
  }
}

/** A typed value: a self-contained schema graph paired with a value tree. */
final case class TypedSchemaValue(graph: SchemaGraph, value: SchemaValue)
