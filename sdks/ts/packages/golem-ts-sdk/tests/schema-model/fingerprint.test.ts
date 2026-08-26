import { describe, expect, test } from 'vitest';
import {
  canonicalSchemaBytesV1,
  schemaFingerprintV1,
} from '../../src/internal/schema-model/fingerprint';
import {
  emptyMetadata,
  type SchemaGraph,
  type SchemaType,
} from '../../src/internal/schema-model/model';

const hex = (bytes: Uint8Array) =>
  [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');
const emptyGraph = (root: SchemaType): SchemaGraph => ({ defs: new Map(), root });
const type = (body: SchemaType['body']): SchemaType => ({ body, metadata: emptyMetadata() });

describe('SchemaFingerprintV1 golden vectors', () => {
  test.each([
    [
      'unit',
      emptyGraph(type({ tag: 'tuple', elements: [] })),
      undefined,
      37,
      'b50494cf0f33961c703d5f6e6af3d3159e528c4d09c1d801172cdf8f022dcafa',
    ],
    [
      'string',
      emptyGraph(type({ tag: 'string' })),
      type({ tag: 'string' }),
      37,
      '61c50c0a3c6ffd63529621ada78afc0d4d8e5fe691f8b0993035f847c660a307',
    ],
    [
      'list<string>',
      emptyGraph(type({ tag: 'list', element: type({ tag: 'string' }) })),
      type({ tag: 'list', element: type({ tag: 'string' }) }),
      45,
      '4939707f8ef97e9d4b31b568332eaf5a3011f2be7c358f7546966fadfb9416d4',
    ],
  ] as const)('%s', (_name, graph, element, length, digest) => {
    expect(canonicalSchemaBytesV1(graph, element)).toHaveLength(length);
    expect(hex(schemaFingerprintV1(graph, element))).toBe(digest);
  });

  test('recursive record', () => {
    const ref = type({ tag: 'ref', id: 'example.node' });
    const graph: SchemaGraph = {
      root: ref,
      defs: new Map([
        [
          'example.node',
          {
            name: 'Node',
            body: type({
              tag: 'record',
              fields: [
                { name: 'value', body: type({ tag: 'string' }), metadata: emptyMetadata() },
                {
                  name: 'next',
                  body: type({ tag: 'option', element: ref }),
                  metadata: emptyMetadata(),
                },
              ],
            }),
          },
        ],
      ]),
    };
    expect(canonicalSchemaBytesV1(graph, graph.root)).toHaveLength(140);
    expect(hex(schemaFingerprintV1(graph, graph.root))).toBe(
      '3931585d2d02a2b7d5c99e3da1082ac8fe904c535e2700bd45e29a95ff2399fa',
    );
  });

  test('constrained text', () => {
    const constrained: SchemaType = {
      body: {
        tag: 'text',
        restrictions: { languages: ['fr', 'en'], minLength: 1, maxLength: 64, regex: '^[a-z]+$' },
      },
      metadata: {
        doc: 'text',
        aliases: ['z', 'a'],
        examples: ['"hello"'],
        deprecated: 'use-v2',
        role: { tag: 'other', val: 'prompt' },
      },
    };
    const bytes = canonicalSchemaBytesV1(emptyGraph(constrained), constrained);
    expect(bytes).toHaveLength(87);
    expect(hex(bytes)).toBe(
      '847818676f6c656d2d736368656d612d66696e6765727072696e74018618198262656e626672011840685e5b612d7a5d2b24856474657874826161617a81672268656c6c6f22667573652d763282036670726f6d707480',
    );
    expect(hex(schemaFingerprintV1(emptyGraph(constrained), constrained))).toBe(
      'b985cdb5445862be90e8dca06bbfa9c46b50cf40edc84ed34205bb3a214c5bb0',
    );
  });

  test('permission-card', () => {
    const card = type({ tag: 'permission-card', spec: { polymorphic: true } });
    expect(hex(canonicalSchemaBytesV1(emptyGraph(card), card))).toBe(
      '847818676f6c656d2d736368656d612d66696e6765727072696e7401831825f585f68080f6f680',
    );
    expect(hex(schemaFingerprintV1(emptyGraph(card), card))).toBe(
      'b7d3c09af5db4e56b527051f561689f451dfdb21213cd70aabd11618e244da8b',
    );
  });
});
