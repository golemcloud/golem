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

// Benchmark for Option 6: memoizing config-property schema generation.
//
// Config (de)serialization (`loadConfigKey`, `serializeRpcConfigObject`) used to
// rebuild a `ResolvedGraph` (via `mapTsTypeToResolvedGraph`) and project it onto
// the wire `SchemaGraph` (via `resolvedGraphToSchemaType`) on *every* call. Both
// outputs depend only on the static `(type, scope)` pair, so `cachedConfigSchema`
// memoizes them.
//
// The `uncached` arm reproduces the previous per-call work; the `cached` arm is
// the current behaviour (warm cache → all hits). Run with `vitest bench` and
// compare the two arms within each group.

import { bench, describe } from 'vitest';
import { buildTypeFromJSON, LiteTypeJSON, Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../src/newTypes/either';
import { mapTsTypeToResolvedGraph } from '../../src/internal/mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from '../../src/internal/mapping/types/schemaType';
import { cachedConfigSchema } from '../../src/internal/mapping/types/configSchemaCache';
import { TypeScope } from '../../src/internal/mapping/types/scope';

const TIME = 1000;

function num(): LiteTypeJSON {
  return { kind: 'number', optional: false };
}

function str(): LiteTypeJSON {
  return { kind: 'string', optional: false };
}

function bool(): LiteTypeJSON {
  return { kind: 'boolean', optional: false };
}

// A simple scalar config property, e.g. `port: number`.
const simpleJson: LiteTypeJSON = num();

// A moderately nested config property, e.g.
// `{ host: string; ports: number[]; tls: { enabled: boolean; ca: string };
//    tags: Record<string, string> }`.
const complexJson: LiteTypeJSON = {
  kind: 'interface',
  name: 'ServerConfig',
  owner: 'M',
  optional: false,
  typeParams: [],
  properties: [
    { name: 'host', type: str() },
    { name: 'ports', type: { kind: 'array', optional: false, element: num() } },
    {
      name: 'tls',
      type: {
        kind: 'interface',
        name: 'TlsConfig',
        owner: 'M',
        optional: false,
        typeParams: [],
        properties: [
          { name: 'enabled', type: bool() },
          { name: 'ca', type: str() },
        ],
      },
    },
    { name: 'tags', type: { kind: 'map', optional: false, typeArgs: [str(), str()] } },
  ],
};

const simpleType: Type.Type = buildTypeFromJSON(simpleJson);
const complexType: Type.Type = buildTypeFromJSON(complexJson);

const simpleScope = TypeScope.object('config', 'port', false);
const complexScope = TypeScope.object('config', 'server', false);

const onError = (err: string) => new Error(`bench mapping failure: ${err}`);

// Reproduces the per-call work removed by Option 6.
function uncached(type: Type.Type, scope: TypeScope): void {
  const graph = Either.getOrThrowWith(mapTsTypeToResolvedGraph(type, scope), onError);
  void resolvedGraphToSchemaType(graph).graph;
}

// Warm the cache so the `cached` arm measures hit cost, not the first miss.
cachedConfigSchema(simpleType, simpleScope, onError);
cachedConfigSchema(complexType, complexScope, onError);

describe('config schema generation: simple scalar', () => {
  bench('uncached (before)', () => uncached(simpleType, simpleScope), { time: TIME });
  bench('cached (after)', () => void cachedConfigSchema(simpleType, simpleScope, onError), {
    time: TIME,
  });
});

describe('config schema generation: nested record', () => {
  bench('uncached (before)', () => uncached(complexType, complexScope), { time: TIME });
  bench('cached (after)', () => void cachedConfigSchema(complexType, complexScope, onError), {
    time: TIME,
  });
});
