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
import scala.collection.mutable

/**
 * Schema-graph construction helper.
 *
 * `SchemaBuilder` registers named (nominal) types into a graph using a
 * reserve/commit protocol so recursive and mutually-recursive types close
 * without infinite recursion: a type reserves its id before walking its body,
 * so a back-reference encountered during the walk short-circuits to a
 * [[SchemaTypeBody.RefType]]. Mirrors the TS SDK's `SchemaBuilder`.
 */
final class SchemaBuilder {
  private final case class Reserved(var name: Option[String], var body: Option[SchemaType])

  // Insertion-ordered so `finish()` is deterministic before id-sorting.
  private val defs = mutable.LinkedHashMap.empty[String, Reserved]

  /** Whether the given id is already reserved or committed. */
  def contains(id: String): Boolean = defs.contains(id)

  /** Reserve a slot for `id` so recursive references can close to a `ref`. */
  def reserve(id: String, name: Option[String] = None): Unit =
    if (!defs.contains(id)) defs.update(id, Reserved(name, None))

  /** Commit the body of a previously reserved (or new) `id`. */
  def commit(id: String, body: SchemaType, name: Option[String] = None): Unit =
    defs.get(id) match {
      case Some(existing) =>
        existing.body = Some(body)
        if (name.isDefined) existing.name = name
      case None =>
        defs.update(id, Reserved(name, Some(body)))
    }

  /** A `ref` schema type pointing at `id`. */
  def ref(id: String): SchemaType = SchemaType(SchemaTypeBody.RefType(id))

  /**
   * Register a nominal type. If `id` is not yet known it is reserved, then
   * `build` is invoked to produce its body (during which recursive references
   * to `id` resolve to a `ref`), then committed. Always returns a `ref` to `id`
   * for use at the call site.
   */
  def register(id: String, build: () => SchemaType, name: Option[String] = None): SchemaType = {
    if (!contains(id)) {
      reserve(id, name)
      commit(id, build(), name)
    }
    ref(id)
  }

  /**
   * Finalize the registered definitions, ensuring every reservation was
   * committed.
   */
  def finish(): ListMap[String, SchemaTypeDef] = {
    val out = ListMap.newBuilder[String, SchemaTypeDef]
    defs.foreach { case (id, def_) =>
      def_.body match {
        case Some(body) => out += (id -> SchemaTypeDef(body, def_.name))
        case None       =>
          throw SchemaEncodeError(s"schema builder: reserved type id '$id' was never committed")
      }
    }
    out.result()
  }

  /**
   * Build a complete graph with the given root and all registered definitions.
   */
  def buildGraph(root: SchemaType): SchemaGraph = SchemaGraph(finish(), root)
}

/**
 * An agent-level merged graph: a shared def registry plus the roots it backs.
 */
final case class MergedAgentGraph(defs: ListMap[String, SchemaTypeDef], roots: List[SchemaType])

object SchemaBuilder {

  /** Build a self-contained graph rooted at the type produced by `build`. */
  def graphOf(build: SchemaBuilder => SchemaType): SchemaGraph = {
    val b    = new SchemaBuilder
    val root = build(b)
    b.buildGraph(root)
  }

  /**
   * Merge definitions from several graphs, rejecting conflicting same-id
   * bodies.
   */
  def mergeGraphDefs(graphs: Iterable[SchemaGraph]): ListMap[String, SchemaTypeDef] = {
    val merged = mutable.LinkedHashMap.empty[String, SchemaTypeDef]
    graphs.foreach { g =>
      g.defs.foreach { case (id, def_) =>
        merged.get(id) match {
          case Some(existing) =>
            if (existing.body != def_.body) throw SchemaConflictError(id)
          case None =>
            merged.update(id, def_)
        }
      }
    }
    val out = ListMap.newBuilder[String, SchemaTypeDef]
    merged.foreach(out += _)
    out.result()
  }

  /**
   * Combine the per-root graphs of an agent (one per constructor / method input
   * / output / config root) into a single deduplicated def registry plus the
   * list of roots, in input order. Conflicting same-id definitions raise
   * [[SchemaConflictError]].
   */
  def mergeAgentGraphs(graphs: List[SchemaGraph]): MergedAgentGraph =
    MergedAgentGraph(mergeGraphDefs(graphs), graphs.map(_.root))
}
