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

// Decode/encode failure paths for the schema-model WIT codecs. These assert
// that malformed flat carriers raise a structured `SchemaDecodeError` /
// `SchemaEncodeError` rather than throwing a generic error or silently
// producing a corrupt value.
//
// Note: the Slice-1 codec is purely structural. It does NOT validate a value
// against its schema, nor union tags / roles / modalities against a schema —
// those checks belong to the later validation slices. The failure modes that
// exist at this layer are out-of-range indices, cycles, duplicate def ids, and
// unknown carrier tags.

import { describe, it, expect } from 'vitest';

import type {
  SchemaGraph as WitSchemaGraph,
  SchemaValueTree as WitSchemaValueTree,
} from 'golem:core/types@2.0.0';

import {
  type SchemaGraph,
  SchemaDecodeError,
  SchemaEncodeError,
  emptyMetadata,
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  t,
} from '../../src/internal/schema-model';

describe('schema value decode failures', () => {
  it('root index out of range -> SchemaDecodeError', () => {
    const wit: WitSchemaValueTree = {
      valueNodes: [{ tag: 'bool-value', val: true }],
      root: 5,
    };
    expect(() => schemaValueFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('child index out of range -> SchemaDecodeError', () => {
    const wit: WitSchemaValueTree = {
      valueNodes: [{ tag: 'record-value', val: [9] }],
      root: 0,
    };
    expect(() => schemaValueFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('cyclic value node reference -> SchemaDecodeError', () => {
    const wit: WitSchemaValueTree = {
      valueNodes: [{ tag: 'list-value', val: [0] }],
      root: 0,
    };
    expect(() => schemaValueFromWit(wit)).toThrow(/cyclic/i);
  });

  it('unknown value node tag -> SchemaDecodeError', () => {
    const wit = {
      valueNodes: [{ tag: 'nonsense-value', val: 1 }],
      root: 0,
    } as unknown as WitSchemaValueTree;
    expect(() => schemaValueFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('unknown result-value payload tag -> SchemaDecodeError (not silently treated as err)', () => {
    const wit = {
      valueNodes: [{ tag: 'result-value', val: { tag: 'nonsense', val: undefined } }],
      root: 0,
    } as unknown as WitSchemaValueTree;
    expect(() => schemaValueFromWit(wit)).toThrow(/unknown result value payload/i);
  });
});

describe('schema graph decode failures', () => {
  it('ref-type pointing at an out-of-range def index -> SchemaDecodeError', () => {
    const wit: WitSchemaGraph = {
      typeNodes: [{ body: { tag: 'ref-type', val: 3 }, metadata: emptyMetadata() }],
      defs: [],
      root: 0,
    };
    expect(() => schemaGraphFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('type node index out of range -> SchemaDecodeError', () => {
    const wit: WitSchemaGraph = {
      typeNodes: [{ body: { tag: 'list-type', val: 9 }, metadata: emptyMetadata() }],
      defs: [],
      root: 0,
    };
    expect(() => schemaGraphFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('duplicate def id -> SchemaDecodeError', () => {
    const meta = emptyMetadata();
    const wit: WitSchemaGraph = {
      typeNodes: [
        { body: { tag: 'bool-type' }, metadata: meta },
        { body: { tag: 'bool-type' }, metadata: meta },
        { body: { tag: 'string-type' }, metadata: meta },
      ],
      defs: [
        { id: 'Dup', body: 0 },
        { id: 'Dup', body: 1 },
      ],
      root: 2,
    };
    expect(() => schemaGraphFromWit(wit)).toThrow(/duplicate def/i);
  });

  it('unknown type body tag -> SchemaDecodeError', () => {
    const wit = {
      typeNodes: [{ body: { tag: 'nonsense-type' }, metadata: emptyMetadata() }],
      defs: [],
      root: 0,
    } as unknown as WitSchemaGraph;
    expect(() => schemaGraphFromWit(wit)).toThrow(SchemaDecodeError);
  });
});

describe('schema graph encode failures', () => {
  it('ref to an unknown type-id -> SchemaEncodeError', () => {
    const graph: SchemaGraph = { defs: new Map(), root: t.ref('does.not.exist') };
    expect(() => schemaGraphToWit(graph)).toThrow(SchemaEncodeError);
  });
});
