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

// Helpers for asserting against the schema-native v2 `AgentType` produced by the
// `@agent` decorator. A v2 agent type carries a single per-agent
// `schema` (a flat WIT `schema-graph` type-node pool); constructor / method /
// config schema roots are `type-node-index` values into that pool. These helpers
// resolve an index back into the recursive in-memory `SchemaType` and normalize
// it into a compact, comparable tree so tests can assert structural shape
// without dealing with the flat index encoding.

import {
  schemaGraphFromWit,
  SchemaType,
  SchemaTypeDef,
  TypeId,
} from '../src/internal/schema-model';
import { AgentType, InputSchema, NamedField } from 'golem:agent/common@2.0.0';
import { TypeNodeIndex } from 'golem:core/types@2.0.0';

/** Ordered parameter names of an input schema. */
export function paramNames(inputSchema: InputSchema): string[] {
  return inputSchema.val.map((f) => f.name);
}

/** Look up a named field of an input schema (throws if absent). */
export function findField(inputSchema: InputSchema, name: string): NamedField {
  const field = inputSchema.val.find((f) => f.name === name);
  if (!field) {
    throw new Error(`Field '${name}' not found in input schema`);
  }
  return field;
}

/**
 * Resolve a type node (by index) within an agent type's `schema` graph back into
 * the recursive in-memory form, preserving `defs` so `ref`s still resolve.
 */
export function schemaTypeAt(
  agentType: AgentType,
  index: TypeNodeIndex,
): { root: SchemaType; defs: Map<TypeId, SchemaTypeDef> } {
  const graph = schemaGraphFromWit({
    typeNodes: agentType.schema.typeNodes,
    defs: agentType.schema.defs,
    root: index,
  });
  return { root: graph.root, defs: graph.defs };
}

function normalizeBody(type: SchemaType, defs: Map<TypeId, SchemaTypeDef>, seen: Set<TypeId>): any {
  const b = type.body;
  switch (b.tag) {
    case 'record':
      return { record: b.fields.map((f) => [f.name, normalizeSchema(f.body, defs, seen)]) };
    case 'variant':
      return {
        variant: b.cases.map((c) => [
          c.name,
          c.payload ? normalizeSchema(c.payload, defs, seen) : null,
        ]),
      };
    case 'enum':
      return { enum: b.cases };
    case 'flags':
      return { flags: b.names };
    case 'option':
      return { option: normalizeSchema(b.element, defs, seen) };
    case 'list':
      return { list: normalizeSchema(b.element, defs, seen) };
    case 'tuple':
      return { tuple: b.elements.map((e) => normalizeSchema(e, defs, seen)) };
    case 'result':
      return {
        result: [
          b.ok ? normalizeSchema(b.ok, defs, seen) : null,
          b.err ? normalizeSchema(b.err, defs, seen) : null,
        ],
      };
    case 'map':
      return { map: [normalizeSchema(b.key, defs, seen), normalizeSchema(b.value, defs, seen)] };
    case 'text':
      return { text: b.restrictions };
    case 'binary':
      return { binary: b.restrictions };
    case 'url':
      return { url: b.restrictions };
    default:
      // Primitives ('string', 'f64', 'bool', 'u8', ...) and other leaves.
      return b.tag;
  }
}

/**
 * Normalize a recursive schema type into a compact, comparable tree. Named defs
 * are rendered as `{ def: <name|null>, ... }`; a back-reference encountered while
 * already resolving a def becomes `{ recRef: <id> }`, which keeps recursive types
 * finite and assertable.
 */
export function normalizeSchema(
  type: SchemaType,
  defs: Map<TypeId, SchemaTypeDef>,
  seen: Set<TypeId> = new Set(),
): any {
  if (type.body.tag === 'ref') {
    const id = type.body.id;
    if (seen.has(id)) {
      return { recRef: id };
    }
    const def = defs.get(id);
    if (!def) {
      throw new Error(`missing def ${id}`);
    }
    const next = new Set(seen);
    next.add(id);
    return { def: def.name ?? null, ...normalizeBody(def.body, defs, next) };
  }
  return normalizeBody(type, defs, seen);
}

/** Normalized structural shape of a named parameter's type. */
export function paramShape(agentType: AgentType, inputSchema: InputSchema, name: string): any {
  const field = findField(inputSchema, name);
  const { root, defs } = schemaTypeAt(agentType, field.schema);
  return normalizeSchema(root, defs);
}

/**
 * The `role` discriminator carried by a named parameter's *root* schema node.
 * A multimodal parameter must carry `role.tag === 'multimodal'` on the `list`
 * node itself (not on the variant cases). Returns `undefined` when no role is set.
 */
export function paramRoleTag(
  agentType: AgentType,
  inputSchema: InputSchema,
  name: string,
): string | undefined {
  const field = findField(inputSchema, name);
  const { root } = schemaTypeAt(agentType, field.schema);
  return root.metadata.role?.tag;
}
