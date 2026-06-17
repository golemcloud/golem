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

import { Type } from '@golemcloud/golem-ts-types-core';
import { TypeScope } from './internal/mapping/types/scope';
import { getConfigValue } from 'golem:agent/host@2.0.0';
import { schemaGraphToWit, schemaValueFromWit } from './internal/schema-model';
import { mapTsTypeToResolvedGraph } from './internal/mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from './internal/mapping/types/schemaType';
import { deserializeGraph } from './internal/mapping/values/schemaValue';
import * as Either from './newTypes/either';

export class Secret<T> {
  private readonly path: string[];
  private readonly type: Type.Type;

  constructor(path: string[], type: Type.Type) {
    this.path = path;
    this.type = type;
  }

  /** Lazily loads or reloads the secret value */
  get(): T {
    return loadConfigKey(this.path, this.type);
  }
}

export class Config<T> {
  constructor(
    readonly properties: Type.ConfigProperty[],
    readonly requiredMembers: { path: string[]; requiredKeys: string[] }[],
  ) {}

  get value(): T {
    return this.loadConfig();
  }

  private loadConfig(): T {
    const root: Record<string, any> = {};

    for (const prop of this.properties) {
      const { path } = prop;
      if (path.length === 0) continue;

      let current = root;
      for (let i = 0; i < path.length - 1; i++) {
        const key = path[i];
        if (!(key in current)) current[key] = {};
        current = current[key];
      }
      current[path.at(-1)!] = prop.secret
        ? new Secret(path, prop.type)
        : loadConfigKey(path, prop.type);
    }

    // Prune nodes where any required child is absent.
    // Already deepest-first from typegen so nested nodes are pruned before parents.
    for (const { path: groupPath, requiredKeys } of this.requiredMembers) {
      let parent: Record<string, any> = root;
      let group: Record<string, any> = root;
      for (const key of groupPath) {
        parent = group;
        group = group[key];
        if (typeof group !== 'object' || group === null) break;
      }
      if (typeof group !== 'object' || group === null) continue;

      if (requiredKeys.some((k) => group[k] == null)) {
        parent[groupPath.at(-1)!] = undefined;
      }
    }

    return root as T;
  }
}

function loadConfigKey(path: string[], type: Type.Type): any {
  const scope = TypeScope.object('config', path.at(-1)!, type.optional);
  const graph = Either.getOrThrowWith(
    mapTsTypeToResolvedGraph(type, scope),
    (err) => new Error(`Failed to analyse config type at path '${path.join('.')}': ${err}`),
  );

  const schemaGraph = resolvedGraphToSchemaType(graph).graph;
  const valueTree = getConfigValue(path, schemaGraphToWit(schemaGraph));

  return deserializeGraph(schemaValueFromWit(valueTree), graph);
}
