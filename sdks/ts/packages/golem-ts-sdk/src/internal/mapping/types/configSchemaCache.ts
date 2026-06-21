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

// Memoizes the schema graph generated for a config property type.
//
// Config (de)serialization (`loadConfigKey`, `serializeRpcConfigObject`)
// reflects a *static* `Type` descriptor into a `ResolvedGraph` via
// `mapTsTypeToResolvedGraph` and projects it onto the wire `SchemaGraph` via
// `resolvedGraphToSchemaType` on every call. Both results depend only on the
// `(type, scope)` pair, which is fixed for a given config property, so we cache
// them keyed by the (stable) `Type` object reference and a serialized scope.
//
// The cached `ResolvedGraph` / `SchemaGraph` are treated as immutable: every
// consumer (the value codec `serialize`/`deserialize`, `resolvedGraphToSchemaType`,
// and `GraphEncoder` behind `schemaGraphToWit`) only reads them.

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../../newTypes/either';
import { mapTsTypeToResolvedGraph } from './resolvedMapper';
import { ResolvedGraph } from './resolvedType';
import { resolvedGraphToSchemaType } from './schemaType';
import { SchemaGraph } from '../../schema-model';
import { TypeScope } from './scope';

/** Memoized schema graphs for a config property type. Do not mutate. */
export interface CachedConfigSchema {
  /** Mapper output, used by the recursion-aware value codec. */
  readonly graph: ResolvedGraph;
  /** Wire projection, used for the typed config value envelope. */
  readonly schemaGraph: SchemaGraph;
}

const cache: WeakMap<Type.Type, Map<string, CachedConfigSchema>> = new WeakMap();

// Injective serialization of every `TypeScope` variant. JSON-encoded tuples
// avoid any delimiter-collision class and stay exhaustive over the union.
function scopeKey(scope: TypeScope): string {
  switch (scope.scope) {
    case 'others':
      return JSON.stringify(['others', scope.name]);
    case 'interface':
    case 'object':
    case 'method':
    case 'constructor':
      return JSON.stringify([scope.scope, scope.name, scope.parameterName, scope.hasQuestionMark]);
  }
}

/**
 * Returns the cached `ResolvedGraph` + wire `SchemaGraph` for `(type, scope)`,
 * generating and storing them on first use.
 *
 * `onError` mirrors the call site's existing `Either.getOrThrowWith` error
 * wrapping; it is only invoked on a generation failure (effectively unreachable
 * at runtime, since config types are validated during agent registration).
 */
export function cachedConfigSchema(
  type: Type.Type,
  scope: TypeScope,
  onError: (err: string) => Error,
): CachedConfigSchema {
  let byScope = cache.get(type);
  if (!byScope) {
    byScope = new Map();
    cache.set(type, byScope);
  }

  const key = scopeKey(scope);
  let entry = byScope.get(key);
  if (!entry) {
    const graph = Either.getOrThrowWith(mapTsTypeToResolvedGraph(type, scope), onError);
    const schemaGraph = resolvedGraphToSchemaType(graph).graph;
    entry = { graph, schemaGraph };
    byScope.set(key, entry);
  }
  return entry;
}
