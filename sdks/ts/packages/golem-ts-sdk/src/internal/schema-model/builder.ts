// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Schema-graph construction helpers.
//
// `SchemaBuilder` registers named (nominal) types into a graph using a
// reserve/commit protocol so recursive and mutually-recursive types close
// without infinite recursion: a type reserves its id before walking its body,
// so a back-reference encountered during the walk short-circuits to a `ref`.
//
// `mergeAgentGraphs` combines the per-root graphs of an agent's constructor,
// methods, and config into a single graph, deduplicating shared definitions by
// stable `type-id` and rejecting divergent definitions of the same id.

import {
  type SchemaGraph,
  type SchemaType,
  type SchemaTypeDef,
  type TypeId,
  deepEqual,
  emptyMetadata,
} from './model';
import { SchemaConflictError } from './errors';

interface ReservedDef {
  name?: string;
  body?: SchemaType;
}

export class SchemaBuilder {
  private readonly defs = new Map<TypeId, ReservedDef>();

  /** Whether the given id is already reserved or committed. */
  contains(id: TypeId): boolean {
    return this.defs.has(id);
  }

  /** Reserve a slot for `id` so recursive references can close to a `ref`. */
  reserve(id: TypeId, name?: string): void {
    if (!this.defs.has(id)) {
      this.defs.set(id, { name, body: undefined });
    }
  }

  /** Commit the body of a previously reserved (or new) `id`. */
  commit(id: TypeId, body: SchemaType, name?: string): void {
    const existing = this.defs.get(id);
    if (existing) {
      existing.body = body;
      if (name !== undefined) existing.name = name;
    } else {
      this.defs.set(id, { name, body });
    }
  }

  /** A `ref` schema type pointing at `id`. */
  ref(id: TypeId): SchemaType {
    return { body: { tag: 'ref', id }, metadata: emptyMetadata() };
  }

  /**
   * Register a nominal type. If `id` is not yet known it is reserved, then
   * `build` is invoked to produce its body (during which recursive references
   * to `id` resolve to a `ref`), then committed. Always returns a `ref` to
   * `id` for use at the call site.
   */
  register(id: TypeId, build: () => SchemaType, name?: string): SchemaType {
    if (!this.contains(id)) {
      this.reserve(id, name);
      this.commit(id, build(), name);
    }
    return this.ref(id);
  }

  /** Finalize the registered definitions, ensuring every reservation was committed. */
  finish(): Map<TypeId, SchemaTypeDef> {
    const out = new Map<TypeId, SchemaTypeDef>();
    for (const [id, def] of this.defs) {
      if (def.body === undefined) {
        throw new Error(`schema builder: reserved type id '${id}' was never committed`);
      }
      out.set(id, { name: def.name, body: def.body });
    }
    return out;
  }

  /** Build a complete graph with the given root and all registered definitions. */
  buildGraph(root: SchemaType): SchemaGraph {
    return { defs: this.finish(), root };
  }
}

/** Merge definitions from several graphs, rejecting conflicting same-id bodies. */
export function mergeGraphDefs(graphs: Iterable<SchemaGraph>): Map<TypeId, SchemaTypeDef> {
  const merged = new Map<TypeId, SchemaTypeDef>();
  for (const g of graphs) {
    for (const [id, def] of g.defs) {
      const existing = merged.get(id);
      if (existing) {
        if (!deepEqual(existing.body, def.body)) {
          throw new SchemaConflictError(id);
        }
      } else {
        merged.set(id, def);
      }
    }
  }
  return merged;
}

/** An agent-level merged graph: a shared def registry plus the roots it backs. */
export interface MergedAgentGraph {
  defs: Map<TypeId, SchemaTypeDef>;
  roots: SchemaType[];
}

/**
 * Combine the per-root graphs of an agent (one per constructor / method input /
 * output / config root) into a single deduplicated def registry plus the list
 * of roots, in input order. Conflicting same-id definitions raise
 * `SchemaConflictError`.
 */
export function mergeAgentGraphs(graphs: SchemaGraph[]): MergedAgentGraph {
  const defs = mergeGraphDefs(graphs);
  const roots = graphs.map((g) => g.root);
  return { defs, roots };
}
