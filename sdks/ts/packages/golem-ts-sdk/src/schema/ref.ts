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

import type { SchemaGraph, SchemaType, SchemaValue } from '../internal/schema-model';
import { schemaValueConforms } from '../internal/tool/validation';
import { fromCanonicalJson, toCanonicalJson, toCanonicalJsonSchema } from './render';

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { readonly [key: string]: JsonValue };

export interface SchemaIssue {
  readonly message: string;
  readonly path: readonly (string | number)[];
}

export type SchemaValidationResult<T> =
  | { readonly success: true; readonly value: T }
  | { readonly success: false; readonly issues: readonly SchemaIssue[] };

export class SchemaRef {
  readonly graph: SchemaGraph;
  readonly root: SchemaType;

  constructor(graph: SchemaGraph, root: SchemaType = graph.root) {
    this.graph = freezeSchemaGraph({ defs: new Map(graph.defs), root });
    this.root = this.graph.root;
    Object.freeze(this);
  }

  validateJson(value: JsonValue): SchemaValidationResult<SchemaValue> {
    try {
      return { success: true, value: this.packJson(value) };
    } catch (error) {
      return { success: false, issues: [schemaIssue(error)] };
    }
  }

  validateValue(value: SchemaValue): SchemaValidationResult<SchemaValue> {
    return schemaValueConforms(this.graph, this.root, value)
      ? { success: true, value }
      : {
          success: false,
          issues: [{ path: [], message: 'schema value does not conform to the expected schema' }],
        };
  }

  packJson(value: JsonValue): SchemaValue {
    return fromCanonicalJson(this.graph, this.root, value);
  }

  unpackJson(value: SchemaValue): JsonValue {
    return toCanonicalJson(this.graph, this.root, value);
  }

  toJsonSchema(options: { includeDraftMarker?: boolean } = {}): JsonValue {
    return toCanonicalJsonSchema(this.graph, this.root, options.includeDraftMarker ?? true);
  }
}

function schemaIssue(error: unknown): SchemaIssue {
  if (error instanceof SchemaRenderError) {
    return { path: error.path, message: error.message };
  }
  return { path: [], message: error instanceof Error ? error.message : String(error) };
}

export class SchemaRenderError extends TypeError {
  constructor(
    message: string,
    public readonly path: readonly (string | number)[] = [],
  ) {
    super(message);
    this.name = 'SchemaRenderError';
  }
}

function freezeSchemaGraph(graph: SchemaGraph): SchemaGraph {
  const seen = new WeakSet<object>();
  const freeze = (value: unknown): void => {
    if (value === null || typeof value !== 'object' || seen.has(value)) return;
    seen.add(value);
    if (value instanceof Map) {
      value.forEach((entryValue, key) => {
        freeze(key);
        freeze(entryValue);
      });
      Object.defineProperties(value, {
        set: { value: immutableGraphMutation },
        delete: { value: immutableGraphMutation },
        clear: { value: immutableGraphMutation },
      });
    } else {
      Reflect.ownKeys(value).forEach((key) => freeze(Reflect.get(value, key)));
    }
    Object.freeze(value);
  };
  freeze(graph);
  return graph;
}

function immutableGraphMutation(): never {
  throw new TypeError('Cannot mutate an immutable reflected schema graph');
}
