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

import type { SchemaGraph } from './model';

/** Clone a caller-owned graph before the SDK turns it into immutable public state. */
export function cloneSchemaGraph(graph: SchemaGraph): SchemaGraph {
  const seen = new WeakMap<object, unknown>();
  return cloneSchemaValue(graph, seen);
}

function cloneSchemaValue<T>(value: T, seen: WeakMap<object, unknown>): T {
  if (value === null || (typeof value !== 'object' && typeof value !== 'function')) return value;

  const object = value as object;
  const existing = seen.get(object);
  if (existing !== undefined) return existing as T;

  if (value instanceof Map) {
    const clone = new Map();
    seen.set(object, clone);
    value.forEach((entryValue, key) => {
      clone.set(cloneSchemaValue(key, seen), cloneSchemaValue(entryValue, seen));
    });
    return clone as T;
  }

  if (Array.isArray(value)) {
    const clone: unknown[] = [];
    seen.set(object, clone);
    value.forEach((entry) => clone.push(cloneSchemaValue(entry, seen)));
    return clone as T;
  }

  if (value instanceof Uint8Array) {
    return value.slice() as T;
  }

  const clone = Object.create(Object.getPrototypeOf(value)) as Record<PropertyKey, unknown>;
  seen.set(object, clone);
  Reflect.ownKeys(value).forEach((key) => {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor) return;
    if ('value' in descriptor) descriptor.value = cloneSchemaValue(descriptor.value, seen);
    Object.defineProperty(clone, key, descriptor);
  });
  return clone as T;
}

/** Recursively freezes a schema graph, including the mutable methods of its definition map. */
export function freezeSchemaGraph(graph: SchemaGraph): SchemaGraph {
  freezeSchemaValue(graph, new WeakSet());
  return graph;
}

export function freezeSchemaValue(value: unknown, seen: WeakSet<object>): void {
  if (value === null || (typeof value !== 'object' && typeof value !== 'function')) return;
  if (seen.has(value)) return;
  seen.add(value);

  if (value instanceof Map) {
    value.forEach((entryValue, key) => {
      freezeSchemaValue(key, seen);
      freezeSchemaValue(entryValue, seen);
    });
    Object.defineProperties(value, {
      set: { value: immutableSchemaMutation },
      delete: { value: immutableSchemaMutation },
      clear: { value: immutableSchemaMutation },
    });
  } else {
    Reflect.ownKeys(value).forEach((key) => {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (descriptor && 'value' in descriptor) freezeSchemaValue(descriptor.value, seen);
    });
  }
  Object.freeze(value);
}

function immutableSchemaMutation(): never {
  throw new TypeError('Cannot mutate an immutable schema graph');
}
