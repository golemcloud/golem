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

import {
  freezeSchemaGraph,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
} from '../internal/schema-model';
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
      const packed = this.packJson(value);
      return this.validateValue(packed);
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
