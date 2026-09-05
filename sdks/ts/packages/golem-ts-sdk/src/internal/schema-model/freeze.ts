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
