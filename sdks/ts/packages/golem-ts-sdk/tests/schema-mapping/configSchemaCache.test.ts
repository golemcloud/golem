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

// `cachedConfigSchema` memoizes the `(ResolvedGraph, SchemaGraph)` pair derived
// from a static config-property `Type` + `TypeScope`. These tests prove:
//   1. the cached result is structurally identical to the uncached
//      `mapTsTypeToResolvedGraph` + `resolvedGraphToSchemaType` path it replaces,
//   2. repeated calls for the same `(type, scope)` return the *same* object
//      (the memoization actually fires),
//   3. distinct scopes for the same `type` object are cached independently,
//   4. distinct `type` objects do not collide.

import { describe, it, expect } from 'vitest';
import { buildTypeFromJSON, LiteTypeJSON } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../src/newTypes/either';
import { mapTsTypeToResolvedGraph } from '../../src/internal/mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from '../../src/internal/mapping/types/schemaType';
import { cachedConfigSchema } from '../../src/internal/mapping/types/configSchemaCache';
import { TypeScope } from '../../src/internal/mapping/types/scope';
import { schemaGraphToWit } from '../../src/internal/schema-model';

function num(): LiteTypeJSON {
  return { kind: 'number', optional: false };
}

function str(): LiteTypeJSON {
  return { kind: 'string', optional: false };
}

// A moderately nested config property type, e.g. `{ a: string; b: number[];
// c: { d: string; e: number } }`, faithfully reconstructed via the runtime path.
function complexConfigJson(name: string): LiteTypeJSON {
  return {
    kind: 'interface',
    name,
    owner: 'M',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'a', type: str() },
      { name: 'b', type: { kind: 'array', optional: false, element: num() } },
      {
        name: 'c',
        type: {
          kind: 'interface',
          name: `${name}Inner`,
          owner: 'M',
          optional: false,
          typeParams: [],
          properties: [
            { name: 'd', type: str() },
            { name: 'e', type: num() },
          ],
        },
      },
    ],
  };
}

const throwOnError = (err: string) => new Error(`unexpected mapping failure: ${err}`);

describe('cachedConfigSchema', () => {
  it('matches the uncached mapping + projection path', () => {
    const type = buildTypeFromJSON(complexConfigJson('Cfg'));
    const scope = TypeScope.object('config', 'settings', false);

    const expectedGraph = Either.getOrThrowWith(
      mapTsTypeToResolvedGraph(type, scope),
      throwOnError,
    );
    const expectedSchemaGraph = resolvedGraphToSchemaType(expectedGraph).graph;

    const cached = cachedConfigSchema(type, scope, throwOnError);

    // Same set of defs and same root in the resolved graph.
    expect([...cached.graph.defs.keys()].sort()).toEqual([...expectedGraph.defs.keys()].sort());
    expect(cached.graph.root).toEqual(expectedGraph.root);

    // Same wire projection (compared via the plain WIT carrier representation).
    expect(schemaGraphToWit(cached.schemaGraph)).toEqual(schemaGraphToWit(expectedSchemaGraph));
  });

  it('returns the same cached object for repeated calls (memoization fires)', () => {
    const type = buildTypeFromJSON(complexConfigJson('CfgRepeat'));
    const scope = TypeScope.object('config', 'settings', false);

    const first = cachedConfigSchema(type, scope, throwOnError);
    const second = cachedConfigSchema(type, scope, throwOnError);

    expect(second).toBe(first);
    expect(second.graph).toBe(first.graph);
    expect(second.schemaGraph).toBe(first.schemaGraph);
  });

  it('caches distinct scopes for the same type object independently', () => {
    const type = buildTypeFromJSON(complexConfigJson('CfgScopes'));

    const required = cachedConfigSchema(
      type,
      TypeScope.object('config', 'settings', false),
      throwOnError,
    );
    const optional = cachedConfigSchema(
      type,
      TypeScope.object('config', 'settings', true),
      throwOnError,
    );
    const otherParam = cachedConfigSchema(
      type,
      TypeScope.object('config', 'other', false),
      throwOnError,
    );

    expect(optional).not.toBe(required);
    expect(otherParam).not.toBe(required);

    // Re-requesting an already-seen scope still hits the same entry.
    expect(
      cachedConfigSchema(type, TypeScope.object('config', 'settings', true), throwOnError),
    ).toBe(optional);
  });

  it('does not collide across distinct type objects', () => {
    const typeA = buildTypeFromJSON(complexConfigJson('CfgA'));
    const typeB = buildTypeFromJSON({ kind: 'string', optional: false });
    const scope = TypeScope.object('config', 'settings', false);

    const a = cachedConfigSchema(typeA, scope, throwOnError);
    const b = cachedConfigSchema(typeB, scope, throwOnError);

    expect(a).not.toBe(b);
    expect(a.graph.root).not.toEqual(b.graph.root);
  });
});
