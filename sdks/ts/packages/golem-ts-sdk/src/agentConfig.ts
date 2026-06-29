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
import { reveal } from 'golem:secrets/reveal@0.1.0';
import type { ResolvedGraph } from './internal/mapping/types/resolvedType';
import { resolvedGraphToSchemaType } from './internal/mapping/types/schemaType';
import {
  drainUnconsumedQuotaHandles,
  GuestSecretHandle,
  preflightWitValueTree,
  schemaGraphToWit,
  type SchemaValue,
} from './internal/schema-model';
import { SECRET_INTERNAL, type SecretInternal } from './internal/schema-model/secretInternal';
import {
  cachedConfigSchema,
  cachedSecretConfigSchema,
} from './internal/mapping/types/configSchemaCache';
import { deserializeGraphFromWit } from './internal/mapping/values/schemaValue';

export class Secret<T> {
  private readonly path?: string[];
  readonly #handle?: GuestSecretHandle;
  private readonly type?: Type.Type;
  private readonly resolvedGraph?: ResolvedGraph;
  private readonly handleOptional: boolean;
  private readonly preservePayloadOptional: boolean;

  constructor(path: string[], type: Type.Type);
  constructor(path: string[], type: Type.Type, handleOptional: boolean);
  constructor(
    path: string[],
    type: Type.Type,
    handleOptional: boolean,
    preservePayloadOptional: boolean,
  );
  constructor(
    pathOrHandle: string[] | GuestSecretHandle,
    typeOrGraph: Type.Type | ResolvedGraph,
    handleOptional?: boolean,
    preservePayloadOptional?: boolean,
  ) {
    if (pathOrHandle instanceof GuestSecretHandle) {
      this.#handle = pathOrHandle;
      this.resolvedGraph = typeOrGraph as ResolvedGraph;
      this.handleOptional = false;
      this.preservePayloadOptional = false;
    } else {
      this.path = pathOrHandle;
      this.type = typeOrGraph as Type.Type;
      this.handleOptional = handleOptional ?? this.type.optional;
      this.preservePayloadOptional = preservePayloadOptional ?? false;
    }
  }

  /** Lazily loads or reloads the secret value */
  get(): T {
    if (this.#handle) {
      return revealSecretHandle(this.#handle, [], this.resolvedGraph!);
    }
    return loadSecretConfigKey(
      this.path!,
      this.type!,
      this.handleOptional,
      this.preservePayloadOptional,
    );
  }

  _toSchemaValue(key: SecretInternal): SchemaValue {
    if (key !== SECRET_INTERNAL) {
      throw new Error('Secret._toSchemaValue is an internal SDK operation');
    }
    if (!this.#handle) {
      const handle = loadSecretConfigHandle(
        this.path!,
        this.type!,
        this.handleOptional,
        false,
        this.preservePayloadOptional,
      );
      if (!handle) {
        throw new Error(`Secret config value at path '${this.path!.join('.')}' is absent`);
      }
      return { tag: 'secret', handle };
    }
    return { tag: 'secret', handle: this.#handle };
  }

  static _fromSchemaValue<T>(
    key: SecretInternal,
    value: SchemaValue,
    graph: ResolvedGraph,
  ): Secret<T> {
    if (key !== SECRET_INTERNAL) {
      throw new Error('Secret._fromSchemaValue is an internal SDK operation');
    }
    if (value.tag !== 'secret') {
      throw new Error(`Expected a secret schema value, got '${value.tag}'`);
    }
    return new Secret(value.handle as unknown as string[], graph as unknown as Type.Type);
  }

  static _fromHandle<T>(
    key: SecretInternal,
    handle: GuestSecretHandle,
    graph: ResolvedGraph,
  ): Secret<T> {
    if (key !== SECRET_INTERNAL) {
      throw new Error('Secret._fromHandle is an internal SDK operation');
    }
    return new Secret(handle as unknown as string[], graph as unknown as Type.Type);
  }

  toJSON(): never {
    throw new Error(
      'secret values cannot be serialized; transfer them through a WIT schema-value-tree',
    );
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
    const propertyPaths = new Set(
      this.properties.filter((prop) => !prop.secret).map((prop) => configPathKey(prop.path)),
    );

    for (const prop of this.properties) {
      const { path } = prop;
      if (path.length === 0) continue;
      const secretHandleOptional =
        prop.secret &&
        (prop.secretHandleOptional ??
          isOptionalSecretHandle(this.requiredMembers, path, prop.type.optional));

      let current = root;
      for (let i = 0; i < path.length - 1; i++) {
        const key = path[i];
        if (!(key in current)) current[key] = {};
        current = current[key];
      }
      current[path.at(-1)!] = prop.secret
        ? secretHandleOptional
          ? loadSecretConfig(path, prop.type, true, prop.secretHandleOptional !== undefined)
          : new Secret(path, prop.type, false, prop.secretHandleOptional !== undefined)
        : loadConfigKey(path, prop.type);
    }

    pruneUndefinedObjects(root, propertyPaths);

    // Prune nodes where any required child is absent.
    // Already deepest-first from typegen so nested nodes are pruned before parents.
    for (const { path: groupPath, requiredKeys } of this.requiredMembers) {
      if (groupPath.length === 0) continue;

      let parent: Record<string, any> = root;
      let group: Record<string, any> = root;
      for (const key of groupPath) {
        parent = group;
        group = group[key];
        if (typeof group !== 'object' || group === null) break;
      }
      if (typeof group !== 'object' || group === null) continue;

      if (requiredKeys.some((k) => group[k] === undefined)) {
        parent[groupPath.at(-1)!] = undefined;
      }
    }

    pruneUndefinedObjects(root, propertyPaths);

    return root as T;
  }
}

function configPathKey(path: readonly string[]): string {
  return path.join('\0');
}

function pruneUndefinedObjects(
  value: Record<string, any>,
  preservePaths: ReadonlySet<string>,
  path: string[] = [],
): boolean {
  if (preservePaths.has(configPathKey(path))) {
    return true;
  }

  let hasPresentValue = false;
  for (const [key, child] of Object.entries(value)) {
    const childPath = [...path, key];
    if (child && typeof child === 'object' && child.constructor === Object) {
      if (!pruneUndefinedObjects(child, preservePaths, childPath)) {
        value[key] = undefined;
      }
    }
    if (value[key] !== undefined || preservePaths.has(configPathKey(childPath))) {
      hasPresentValue = true;
    }
  }
  return hasPresentValue;
}

function isRequiredConfigPath(
  requiredMembers: readonly { path: string[]; requiredKeys: string[] }[],
  path: readonly string[],
): boolean {
  const parentPath = path.slice(0, -1);
  const key = path.at(-1);
  return requiredMembers.some(
    (entry) =>
      entry.path.length === parentPath.length &&
      entry.path.every((part, i) => part === parentPath[i]) &&
      key !== undefined &&
      entry.requiredKeys.includes(key),
  );
}

function isOptionalSecretHandle(
  requiredMembers: readonly { path: string[]; requiredKeys: string[] }[],
  path: readonly string[],
  typeOptional: boolean,
): boolean {
  if (!typeOptional) return false;
  if (!isRequiredConfigPath(requiredMembers, path)) return true;

  for (let length = 1; length < path.length; length++) {
    if (!isRequiredConfigPath(requiredMembers, path.slice(0, length))) return true;
  }

  return false;
}

function loadConfigKey(path: string[], type: Type.Type): any {
  const scope = TypeScope.object('config', path.at(-1)!, type.optional);
  const { graph, schemaGraph } = cachedConfigSchema(
    type,
    scope,
    (err) => new Error(`Failed to analyse config type at path '${path.join('.')}': ${err}`),
  );

  const valueTree = getConfigValue(path, schemaGraphToWit(schemaGraph));

  return deserializeGraphFromWit(valueTree, graph);
}

function loadSecretConfigKey(
  path: string[],
  type: Type.Type,
  handleOptional: boolean,
  preservePayloadOptional = false,
): any {
  const scope = TypeScope.object('config', path.at(-1)!, type.optional);
  const { graph, schemaGraph } = cachedSecretConfigSchema(
    type,
    scope,
    (err) => new Error(`Failed to analyse secret config type at path '${path.join('.')}': ${err}`),
    handleOptional,
    preservePayloadOptional,
  );

  const handle = loadSecretConfigHandle(path, type, handleOptional, false, preservePayloadOptional);
  if (!handle) {
    if (handleOptional) return undefined;
    throw new Error(`Secret config value at path '${path.join('.')}' is absent`);
  }

  const result = revealSecretHandle(handle, path, graph, schemaGraph);
  if (handle.take() === undefined) {
    throw new Error(`Secret config handle at path '${path.join('.')}' was already transferred`);
  }
  return result;
}

function loadSecretConfig(
  path: string[],
  type: Type.Type,
  handleOptional: boolean,
  preservePayloadOptional = false,
): Secret<any> | undefined {
  const scope = TypeScope.object('config', path.at(-1)!, type.optional);
  const { secretSchemaGraph } = cachedSecretConfigSchema(
    type,
    scope,
    (err) => new Error(`Failed to analyse secret config type at path '${path.join('.')}': ${err}`),
    handleOptional,
    preservePayloadOptional,
  );

  const handle = readSecretConfigHandle(path, secretSchemaGraph, handleOptional, false);
  if (!handle) {
    if (handleOptional) return undefined;
    throw new Error(`Secret config value at path '${path.join('.')}' is absent`);
  }
  return new Secret(path, type, handleOptional, preservePayloadOptional);
}

function loadSecretConfigHandle(
  path: string[],
  type: Type.Type,
  handleOptional: boolean,
  consumeSource = true,
  preservePayloadOptional = false,
): GuestSecretHandle | undefined {
  const scope = TypeScope.object('config', path.at(-1)!, type.optional);
  const { secretSchemaGraph } = cachedSecretConfigSchema(
    type,
    scope,
    (err) => new Error(`Failed to analyse secret config type at path '${path.join('.')}': ${err}`),
    handleOptional,
    preservePayloadOptional,
  );

  return readSecretConfigHandle(path, secretSchemaGraph, handleOptional, consumeSource);
}

function readSecretConfigHandle(
  path: string[],
  secretSchemaGraph: ReturnType<typeof cachedSecretConfigSchema>['secretSchemaGraph'],
  handleOptional: boolean,
  consumeSource: boolean,
): GuestSecretHandle | undefined {
  const value = getConfigValue(path, schemaGraphToWit(secretSchemaGraph));
  try {
    preflightWitValueTree(value.valueNodes, value.root);
  } catch (e) {
    drainUnconsumedQuotaHandles(value.valueNodes);
    throw e;
  }

  const root = value.valueNodes[value.root];
  if (root === undefined) {
    throwSecretConfigReadError(
      value,
      `Secret config value at path '${path.join('.')}' has an invalid root`,
    );
  }
  if (root.tag === 'option-value') {
    if (!handleOptional) {
      throwSecretConfigReadError(
        value,
        `Expected secret config value at path '${path.join('.')}', got 'option-value'`,
      );
    }
    if (root.val === undefined) return undefined;
    const inner = value.valueNodes[root.val];
    if (inner?.tag !== 'secret-value') {
      throwSecretConfigReadError(
        value,
        `Expected optional secret config value at path '${path.join('.')}', got option<${inner?.tag ?? 'missing'}>`,
      );
    }
    const raw = inner.val;
    if (consumeSource) {
      (inner as { val: unknown }).val = undefined;
      return GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw);
    }
    return GuestSecretHandle.fromRawWithTakeCallback(SECRET_INTERNAL, raw, () => {
      (inner as { val: unknown }).val = undefined;
    });
  }
  if (root.tag !== 'secret-value') {
    throwSecretConfigReadError(
      value,
      `Expected secret config value at path '${path.join('.')}', got '${root.tag}'`,
    );
  }
  const raw = root.val;
  if (consumeSource) {
    (root as { val: unknown }).val = undefined;
    return GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw);
  }
  return GuestSecretHandle.fromRawWithTakeCallback(SECRET_INTERNAL, raw, () => {
    (root as { val: unknown }).val = undefined;
  });
}

function throwSecretConfigReadError(
  value: ReturnType<typeof getConfigValue>,
  message: string,
): never {
  drainUnconsumedQuotaHandles(value.valueNodes);
  throw new Error(message);
}

function revealSecretHandle(
  handle: GuestSecretHandle,
  path: string[],
  graph: ResolvedGraph,
  cachedSchemaGraph?: ReturnType<typeof cachedSecretConfigSchema>['schemaGraph'],
): any {
  const schemaGraph = cachedSchemaGraph ?? resolvedGraphToSchemaType(graph).graph;

  const revealed = handle.withHandle((raw) => reveal(raw, schemaGraphToWit(schemaGraph)));
  if (revealed === undefined) {
    throw new Error(`Secret config handle at path '${path.join('.')}' was already transferred`);
  }

  return deserializeGraphFromWit(revealed, graph);
}
