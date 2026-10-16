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
  cloneSchemaGraph,
  cloneSchemaValue,
  freezeSchemaGraph,
  freezeSchemaValue,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
} from '../internal/schema-model';
import { schemaValueConforms } from '../internal/tool/validation';
import {
  fromCanonicalJson,
  toCanonicalJson,
  toCanonicalJsonSchema,
  type RecordFieldDefaults,
} from './render';

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
  private readonly fieldDefaults?: RecordFieldDefaults;

  constructor(graph: SchemaGraph, root: SchemaType = graph.root) {
    this.graph = freezeSchemaGraph(cloneSchemaGraph({ defs: graph.defs, root }));
    this.root = this.graph.root;
    Object.freeze(this);
  }

  /**
   * @internal Build a view over definitions already owned and frozen by the SDK.
   *
   * `fieldDefaults` applies only to the JSON surface of an inline root record: canonical JSON
   * may omit those fields, and the rendered JSON Schema does not require them. Packed values
   * always contain every field of the record.
   */
  static fromImmutableGraph(
    graph: SchemaGraph,
    root: SchemaType,
    fieldDefaults?: RecordFieldDefaults,
  ): SchemaRef {
    freezeSchemaValue(root, new WeakSet());
    if (fieldDefaults !== undefined) {
      if (root.body.tag !== 'record')
        throw new TypeError('Field defaults require an inline record root');
    }
    const ref = Object.create(SchemaRef.prototype) as SchemaRef;
    Object.defineProperties(ref, {
      graph: { value: Object.freeze({ defs: graph.defs, root }), enumerable: true },
      root: { value: root, enumerable: true },
      ...(fieldDefaults === undefined
        ? {}
        : {
            fieldDefaults: {
              value: new Map(
                [...fieldDefaults].map(([name, value]) => [name, cloneSchemaValue(value)]),
              ),
            },
          }),
    });
    return Object.freeze(ref);
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

  /** Pack canonical JSON and validate it against the complete schema. */
  packJson(value: JsonValue): SchemaValue {
    const packed = fromCanonicalJson(this.graph, this.root, value, [], this.fieldDefaults);
    if (!schemaValueConforms(this.graph, this.root, packed)) {
      throw new SchemaRenderError('schema value does not conform to the expected schema');
    }
    return packed;
  }

  unpackJson(value: SchemaValue): JsonValue {
    if (!schemaValueConforms(this.graph, this.root, value)) {
      throw new SchemaRenderError('schema value does not conform to the expected schema');
    }
    return toCanonicalJson(this.graph, this.root, value);
  }

  toJsonSchema(options: { includeDraftMarker?: boolean } = {}): JsonValue {
    return toCanonicalJsonSchema(
      this.graph,
      this.root,
      options.includeDraftMarker ?? true,
      this.fieldDefaults,
    );
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
