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
import * as WitValue from './internal/mapping/values/WitValue';
import * as WitType from './internal/mapping/types/WitType';
import { getConfigValue } from 'golem:agent/host@1.5.0';
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
  constructor(readonly properties: Type.ConfigProperty[]) {}

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

      const leafKey = path[path.length - 1];
      let leafValue;
      if (prop.secret) {
        leafValue = new Secret(path, prop.type);
      } else {
        leafValue = loadConfigKey(path, prop.type);
      }

      current[leafKey] = leafValue;
    }

    return root as T;
  }
}

function loadConfigKey(path: string[], type: Type.Type): any {
  const [witType, analysedType] = Either.getOrThrowWith(
    WitType.fromTsType(type, undefined),
    (err) => new Error(`Failed to analyse config type: ${err}`),
  );

  let witValue = getConfigValue(path, witType);

  return WitValue.toTsValue(witValue, analysedType);
}
