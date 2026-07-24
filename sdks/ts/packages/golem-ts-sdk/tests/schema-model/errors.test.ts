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
  SchemaValueNode as WitSchemaValueNode,
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
  it('rejects sparse WIT list carriers', () => {
    const fields = new Array<number>(1);
    const wit = {
      valueNodes: [{ tag: 'record-value', val: fields }],
      root: 0,
    } as WitSchemaValueTree;

    expect(() => schemaValueFromWit(wit)).toThrow(SchemaDecodeError);
  });

  it('preflights every value-node carrier before decoding', () => {
    type NodeTag = WitSchemaValueNode['tag'];
    type Case = {
      valid: () => WitSchemaValueNode;
      malformed?: () => unknown[];
    };
    const leaf = { tag: 'string-value', val: 'child' } as const;
    const cases = {
      'bool-value': { valid: () => ({ tag: 'bool-value', val: true }) },
      's8-value': { valid: () => ({ tag: 's8-value', val: 0 }) },
      's16-value': { valid: () => ({ tag: 's16-value', val: 0 }) },
      's32-value': { valid: () => ({ tag: 's32-value', val: 0 }) },
      's64-value': { valid: () => ({ tag: 's64-value', val: 0n }) },
      'u8-value': { valid: () => ({ tag: 'u8-value', val: 0 }) },
      'u16-value': { valid: () => ({ tag: 'u16-value', val: 0 }) },
      'u32-value': { valid: () => ({ tag: 'u32-value', val: 0 }) },
      'u64-value': { valid: () => ({ tag: 'u64-value', val: 0n }) },
      'f32-value': { valid: () => ({ tag: 'f32-value', val: 0 }) },
      'f64-value': { valid: () => ({ tag: 'f64-value', val: 0 }) },
      'char-value': { valid: () => ({ tag: 'char-value', val: 'x' }) },
      'string-value': { valid: () => ({ tag: 'string-value', val: '' }) },
      'record-value': {
        valid: () => ({ tag: 'record-value', val: [] }),
        malformed: () => [{ tag: 'record-value', val: null }],
      },
      'variant-value': {
        valid: () => ({ tag: 'variant-value', val: { case_: 0, payload: undefined } }),
        malformed: () => [
          { tag: 'variant-value', val: null },
          { tag: 'variant-value', val: { case_: 0, payload: 'bad' } },
        ],
      },
      'enum-value': { valid: () => ({ tag: 'enum-value', val: 0 }) },
      'flags-value': { valid: () => ({ tag: 'flags-value', val: [] }) },
      'tuple-value': {
        valid: () => ({ tag: 'tuple-value', val: [] }),
        malformed: () => [{ tag: 'tuple-value', val: null }],
      },
      'list-value': {
        valid: () => ({ tag: 'list-value', val: [] }),
        malformed: () => [{ tag: 'list-value', val: [0.5] }],
      },
      'fixed-list-value': {
        valid: () => ({ tag: 'fixed-list-value', val: [] }),
        malformed: () => [{ tag: 'fixed-list-value', val: {} }],
      },
      'map-value': {
        valid: () => ({ tag: 'map-value', val: [] }),
        malformed: () => [
          { tag: 'map-value', val: null },
          { tag: 'map-value', val: [null] },
          { tag: 'map-value', val: [{ key: 1 }] },
        ],
      },
      'option-value': { valid: () => ({ tag: 'option-value', val: undefined }) },
      'result-value': {
        valid: () => ({ tag: 'result-value', val: { tag: 'ok-value', val: undefined } }),
        malformed: () => [
          { tag: 'result-value', val: null },
          { tag: 'result-value', val: { tag: 'ok-value', val: 'bad' } },
        ],
      },
      'text-value': {
        valid: () => ({ tag: 'text-value', val: { text: '', language: undefined } }),
        malformed: () => [
          { tag: 'text-value', val: null },
          { tag: 'text-value', val: { text: 1 } },
        ],
      },
      'binary-value': {
        valid: () => ({
          tag: 'binary-value',
          val: { bytes: new Uint8Array(), mimeType: undefined },
        }),
        malformed: () => [
          { tag: 'binary-value', val: null },
          { tag: 'binary-value', val: { bytes: [] } },
        ],
      },
      'path-value': { valid: () => ({ tag: 'path-value', val: '' }) },
      'url-value': { valid: () => ({ tag: 'url-value', val: '' }) },
      'datetime-value': {
        valid: () => ({ tag: 'datetime-value', val: { seconds: -(1n << 63n), nanoseconds: 0 } }),
        malformed: () => [
          { tag: 'datetime-value', val: null },
          { tag: 'datetime-value', val: { seconds: 0, nanoseconds: 0 } },
          { tag: 'datetime-value', val: { seconds: -(1n << 63n) - 1n, nanoseconds: 0 } },
          { tag: 'datetime-value', val: { seconds: 1n << 63n, nanoseconds: 0 } },
          { tag: 'datetime-value', val: { seconds: 0n, nanoseconds: 0.5 } },
          { tag: 'datetime-value', val: { seconds: 0n, nanoseconds: -1 } },
          { tag: 'datetime-value', val: { seconds: 0n, nanoseconds: 1_000_000_000 } },
        ],
      },
      'duration-value': {
        valid: () => ({ tag: 'duration-value', val: { nanoseconds: (1n << 63n) - 1n } }),
        malformed: () => [
          { tag: 'duration-value', val: null },
          { tag: 'duration-value', val: { nanoseconds: -(1n << 63n) - 1n } },
          { tag: 'duration-value', val: { nanoseconds: 1n << 63n } },
        ],
      },
      'quantity-value-node': {
        valid: () => ({
          tag: 'quantity-value-node',
          val: { mantissa: (1n << 63n) - 1n, scale: 0, unit: '' },
        }),
        malformed: () => [
          { tag: 'quantity-value-node', val: null },
          { tag: 'quantity-value-node', val: { mantissa: 0n, scale: 0 } },
          {
            tag: 'quantity-value-node',
            val: { mantissa: -(1n << 63n) - 1n, scale: 0, unit: '' },
          },
          { tag: 'quantity-value-node', val: { mantissa: 1n << 63n, scale: 0, unit: '' } },
        ],
      },
      'union-value': {
        valid: () => ({ tag: 'union-value', val: { tag: '', body: 1 } }),
        malformed: () => [
          { tag: 'union-value', val: null },
          { tag: 'union-value', val: { tag: '', body: 'bad' } },
        ],
      },
      'secret-value': {
        valid: () => ({ tag: 'secret-value', val: {} as never }),
      },
      'quota-token-handle': {
        valid: () => ({ tag: 'quota-token-handle', val: {} as never }),
      },
    } satisfies Record<NodeTag, Case>;

    for (const [tag, testCase] of Object.entries(cases)) {
      const root = testCase.valid();
      const valueNodes: WitSchemaValueNode[] = tag === 'union-value' ? [root, leaf] : [root];
      expect(() => schemaValueFromWit({ valueNodes, root: 0 }), `${tag} valid`).not.toThrow();

      if ('malformed' in testCase) {
        for (const malformed of testCase.malformed()) {
          let thrown: unknown;
          try {
            schemaValueFromWit({ valueNodes: [malformed] as WitSchemaValueNode[], root: 0 });
          } catch (error) {
            thrown = error;
          }
          expect(thrown, `${tag} malformed carrier`).toBeInstanceOf(SchemaDecodeError);
          expect(thrown).not.toBeInstanceOf(TypeError);
        }
      }
    }
  });

  it('rejects malformed unreachable slots and still clears later quota handles', () => {
    const quotaNode = { tag: 'quota-token-handle', val: {} as never } as const;
    const wit = {
      valueNodes: [{ tag: 'string-value', val: 'root' }, null, quotaNode],
      root: 0,
    } as unknown as WitSchemaValueTree;

    let thrown: unknown;
    try {
      schemaValueFromWit(wit);
    } catch (error) {
      thrown = error;
    }

    expect(thrown).toBeInstanceOf(SchemaDecodeError);
    expect(thrown).not.toBeInstanceOf(TypeError);
    expect(quotaNode.val).toBeUndefined();
  });

  // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
  it('rejects unreachable value nodes outside their WIT scalar domains', () => {
    const wit = {
      valueNodes: [
        { tag: 'bool-value', val: true },
        { tag: 's8-value', val: 128 },
      ],
      root: 0,
    } as WitSchemaValueTree;

    expect(() => schemaValueFromWit(wit)).toThrow(SchemaDecodeError);
  });

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

  // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
  it('rejects schema graph fields outside their declared WIT scalar domains', () => {
    const wit = {
      typeNodes: [
        {
          body: { tag: 'fixed-list-type', val: { element: 1, length: 4_294_967_296 } },
          metadata: emptyMetadata(),
        },
        { body: { tag: 'u8-type', val: undefined }, metadata: emptyMetadata() },
      ],
      defs: [],
      root: 0,
    } as WitSchemaGraph;

    expect(() => schemaGraphFromWit(wit)).toThrow(SchemaDecodeError);
  });
});

describe('schema graph encode failures', () => {
  it('ref to an unknown type-id -> SchemaEncodeError', () => {
    const graph: SchemaGraph = { defs: new Map(), root: t.ref('does.not.exist') };
    expect(() => schemaGraphToWit(graph)).toThrow(SchemaEncodeError);
  });
});
