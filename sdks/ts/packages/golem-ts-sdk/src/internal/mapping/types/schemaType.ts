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

// Projection of the SDK's in-memory `ResolvedType` onto the wire schema model
// (`SchemaType` / `SchemaGraph`). Representation hints carried by `ResolvedType`
// are intentionally dropped here — they never reach the wire. Named, nominal
// composites (`record` / `variant` / `enum` / `flags`) are registered as graph
// definitions keyed by a stable `type-id` derived from their owner + name, so
// that shared types are deduplicated to a single `ref`; everything else is
// inlined.

import { Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../../newTypes/either';
import { TypeScope } from './scope';
import { typeMapper } from './typeMapperImpl';
import { analysedToResolved } from './analysedToResolved';
import { ResolvedGraph, ResolvedType } from './resolvedType';
import {
  field as schemaField,
  SchemaBuilder,
  SchemaConflictError,
  SchemaGraph,
  SchemaType,
  t,
  TypeId,
  variantCase,
} from '../../schema-model';

export interface SchemaTypeMapping {
  graph: SchemaGraph;
  root: SchemaType;
  resolved: ResolvedType;
}

export interface SchemaGraphMapping {
  graph: SchemaGraph;
  root: SchemaType;
  resolvedGraph: ResolvedGraph;
}

// Names that denote built-in / generic containers rather than nominal user
// types. They must never become graph definitions (their structural shape, e.g.
// `Map<string, number>` vs `Map<string, string>`, would otherwise collide on a
// single `type-id`).
const BUILTIN_GENERIC_NAMES = new Set<string>([
  'Array',
  'ReadonlyArray',
  'Map',
  'ReadonlyMap',
  'Set',
  'ReadonlySet',
  'Promise',
  'Record',
  'Result',
  'Option',
  'Uint8Array',
  'Uint16Array',
  'Uint32Array',
  'BigUint64Array',
  'Int8Array',
  'Int16Array',
  'Int32Array',
  'BigInt64Array',
  'Float32Array',
  'Float64Array',
]);

/**
 * Stable `type-id` for a nominal composite identified by `name` (+ optional
 * `owner`). Returns `undefined` for anonymous types and for built-in / generic
 * container names, which must never become graph definitions. Shared by the
 * projection and by the `ResolvedType`-native mapper so both agree on ids.
 */
export function typeIdForName(
  name: string | undefined,
  owner: string | undefined,
): TypeId | undefined {
  if (!name) return undefined;
  if (BUILTIN_GENERIC_NAMES.has(name)) return undefined;
  return owner ? `${owner}.${name}` : name;
}

function typeIdFor(rt: ResolvedType): TypeId | undefined {
  return typeIdForName(rt.name, rt.owner);
}

/** Convert a `ResolvedType` into a `SchemaType`, registering nominal defs into `builder`. */
export function resolvedToSchemaType(resolved: ResolvedType): SchemaTypeMapping {
  const builder = new SchemaBuilder();
  // Tracks the source `ResolvedType` registered under each id, so that two
  // distinct types sharing the same owner+name are reported as a conflict
  // instead of being silently aliased.
  const registered = new Map<TypeId, string>();

  const toSchema = (rt: ResolvedType): SchemaType => {
    const body = rt.body;
    switch (body.tag) {
      case 'bool':
        return t.bool();
      case 's8':
        return t.s8();
      case 's16':
        return t.s16();
      case 's32':
        return t.s32();
      case 's64':
        return t.s64();
      case 'u8':
        return t.u8();
      case 'u16':
        return t.u16();
      case 'u32':
        return t.u32();
      case 'u64':
        return t.u64();
      case 'f32':
        return t.f32();
      case 'f64':
        return t.f64();
      case 'char':
        return t.char();
      case 'string':
        return t.string();
      case 'list':
        return t.list(toSchema(body.element));
      case 'map':
        return t.map(toSchema(body.key), toSchema(body.value));
      case 'tuple':
        return t.tuple(body.elements.map(toSchema));
      case 'option':
        return t.option(toSchema(body.element));
      case 'result':
        return t.result(
          body.ok ? toSchema(body.ok) : undefined,
          body.err ? toSchema(body.err) : undefined,
        );
      case 'record':
        return registerOrInline(rt, () =>
          t.record(body.fields.map((f) => schemaField(f.name, toSchema(f.type)))),
        );
      case 'variant':
        return registerOrInline(rt, () =>
          t.variant(
            body.cases.map((c) => variantCase(c.name, c.payload ? toSchema(c.payload) : undefined)),
          ),
        );
      case 'enum':
        return registerOrInline(rt, () => t.enum(body.cases));
      case 'flags':
        return registerOrInline(rt, () => t.flags(body.names));
      case 'ref':
        // The legacy-lift path (`analysedToResolved`) never produces refs;
        // recursive graphs go through `resolvedGraphToSchemaType` instead.
        throw new Error(`internal error: unexpected ref '${body.id}' in non-graph projection`);
    }
  };

  const registerOrInline = (rt: ResolvedType, build: () => SchemaType): SchemaType => {
    const id = typeIdFor(rt);
    if (id === undefined) {
      return build();
    }
    const hash = JSON.stringify(rt);
    const prev = registered.get(id);
    if (prev !== undefined) {
      if (prev !== hash) {
        throw new SchemaConflictError(id);
      }
      return builder.ref(id);
    }
    registered.set(id, hash);
    return builder.register(id, build, rt.name);
  };

  const root = toSchema(resolved);
  return { graph: builder.buildGraph(root), root, resolved };
}

/**
 * Project a `ResolvedGraph` (produced by the `ResolvedType`-native mapper) onto
 * the wire schema model. Unlike `resolvedToSchemaType`, this does NOT discover
 * definitions: the mapper already owns def discovery / dedup / recursion, so
 * this projector is purely mechanical. Named composite occurrences arrive as
 * `ref`s (closing recursive cycles); only anonymous composites are inlined.
 */
export function resolvedGraphToSchemaType(graph: ResolvedGraph): SchemaGraphMapping {
  const builder = new SchemaBuilder();

  // Reserve every mapper-owned def first so refs (including forward / recursive
  // ones) close to a `ref` during conversion.
  for (const [id, def] of graph.defs) {
    builder.reserve(id, def.name);
  }

  const toSchema = (rt: ResolvedType): SchemaType => {
    const body = rt.body;
    switch (body.tag) {
      case 'bool':
        return t.bool();
      case 's8':
        return t.s8();
      case 's16':
        return t.s16();
      case 's32':
        return t.s32();
      case 's64':
        return t.s64();
      case 'u8':
        return t.u8();
      case 'u16':
        return t.u16();
      case 'u32':
        return t.u32();
      case 'u64':
        return t.u64();
      case 'f32':
        return t.f32();
      case 'f64':
        return t.f64();
      case 'char':
        return t.char();
      case 'string':
        return t.string();
      case 'list':
        return t.list(toSchema(body.element));
      case 'map':
        return t.map(toSchema(body.key), toSchema(body.value));
      case 'tuple':
        return t.tuple(body.elements.map(toSchema));
      case 'option':
        return t.option(toSchema(body.element));
      case 'result':
        return t.result(
          body.ok ? toSchema(body.ok) : undefined,
          body.err ? toSchema(body.err) : undefined,
        );
      case 'record':
        return t.record(body.fields.map((f) => schemaField(f.name, toSchema(f.type))));
      case 'variant':
        return t.variant(
          body.cases.map((c) => variantCase(c.name, c.payload ? toSchema(c.payload) : undefined)),
        );
      case 'enum':
        return t.enum(body.cases);
      case 'flags':
        return t.flags(body.names);
      case 'ref':
        if (!graph.defs.has(body.id)) {
          throw new Error(`internal error: dangling resolved ref '${body.id}'`);
        }
        return builder.ref(body.id);
    }
  };

  for (const [id, def] of graph.defs) {
    builder.commit(id, toSchema(def), def.name);
  }

  const root = toSchema(graph.root);
  return { graph: builder.buildGraph(root), root, resolvedGraph: graph };
}

/**
 * Map a reflected TypeScript type to a self-contained schema graph plus its
 * (hint-carrying) `ResolvedType`. The `resolved` value is what the runtime value
 * codec uses; the `graph`/`root` are the wire schema.
 */
export function fromTsType(
  type: CoreType.Type,
  scope: TypeScope | undefined,
): Either.Either<SchemaTypeMapping, string> {
  return Either.map(typeMapper(type, scope), (analysed) =>
    resolvedToSchemaType(analysedToResolved(analysed)),
  );
}
