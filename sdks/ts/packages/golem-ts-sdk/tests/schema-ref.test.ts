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

import { describe, expect, it } from 'vitest';
import {
  emptyMetadata,
  field,
  schemaType,
  t,
  v,
  type SchemaGraph,
} from '../src/internal/schema-model';
import { SchemaRef } from '../src/schema/ref';

function schema(root: SchemaGraph['root']): SchemaRef {
  return new SchemaRef({ defs: new Map(), root });
}

describe('SchemaRef canonical JSON', () => {
  it('uses the canonical object representation for text', () => {
    const ref = schema(
      schemaType({
        tag: 'text',
        restrictions: { minLength: 2, maxLength: 5, languages: ['en'] },
      }),
    );

    expect(ref.packJson({ text: 'hello', language: 'en' })).toEqual(v.text('hello', 'en'));
    expect(ref.unpackJson(v.text('hello'))).toEqual({ text: 'hello' });
    expect(ref.validateJson({ text: 'x', language: 'en' }).success).toBe(false);
    expect(ref.validateJson({ text: 'hello', language: 'de' }).success).toBe(false);
    expect(ref.validateJson('hello').success).toBe(false);
    expect(ref.validateJson({ text: 'hello', extra: true }).success).toBe(false);
  });

  it('round-trips binary values as unpadded base64url and validates MIME types', () => {
    const ref = schema(
      schemaType({
        tag: 'binary',
        restrictions: { minBytes: 2, maxBytes: 3, mimeTypes: ['application/octet-stream'] },
      }),
    );
    const json = { bytes: '-_8', mimeType: 'application/octet-stream' } as const;

    expect(ref.packJson(json)).toEqual(
      v.binary(Uint8Array.from([251, 255]), 'application/octet-stream'),
    );
    expect(ref.unpackJson(ref.packJson(json))).toEqual(json);
    expect(ref.validateJson({ bytes: 'AQ==', mimeType: 'application/octet-stream' })).toEqual({
      success: false,
      issues: [{ path: ['bytes'], message: 'invalid base64url without padding' }],
    });
    expect(ref.validateJson({ bytes: 'AQI', mimeType: 'not a mime' }).success).toBe(false);
  });

  it('accepts ISO 8601 and shorthand durations and emits ISO 8601', () => {
    const ref = schema(t.duration());

    expect(ref.packJson('PT1M2.003S')).toEqual(v.duration(62_003_000_000n));
    expect(ref.packJson('250ms')).toEqual(v.duration(250_000_000n));
    expect(ref.unpackJson(v.duration(-90_000_000_000n))).toBe('-PT1M30S');
  });

  it('uses JSON integers for quantity mantissas', () => {
    const ref = schema(t.quantity({ baseUnit: 'm', allowedUnits: [] }));
    const json = { mantissa: 123, scale: -2, unit: 'm' } as const;

    expect(ref.packJson(json)).toEqual(v.quantity({ mantissa: 123n, scale: -2, unit: 'm' }));
    expect(ref.unpackJson(ref.packJson(json))).toEqual(json);
    expect(ref.validateJson({ ...json, mantissa: '123' }).success).toBe(false);
  });

  it('rejects unknown and duplicate flags', () => {
    const ref = schema(t.flags(['read', 'write']));

    expect(ref.validateJson(['read', 'read']).success).toBe(false);
    expect(ref.validateJson(['admin']).success).toBe(false);
  });

  it('keeps structural packing separate from full restriction validation', () => {
    const ref = schema(t.u32({ min: { tag: 'unsigned', val: 5n } }));

    expect(ref.packJson(3)).toEqual(v.u32(3));
    expect(ref.validateJson(3)).toEqual({
      success: false,
      issues: [{ path: [], message: 'schema value does not conform to the expected schema' }],
    });
  });
});

describe('SchemaRef JSON Schema', () => {
  it('renders canonical records, optional fields, metadata, and exact tuples', () => {
    const named = t.record([
      field('name', t.string(), {
        ...emptyMetadata(),
        doc: 'Display name',
        examples: ['Ada'],
      }),
      field('nickname', t.option(t.string())),
    ]);
    const graph: SchemaGraph = {
      defs: new Map([['person', { name: 'Person', body: named }]]),
      root: t.tuple([t.ref('person'), t.u8()]),
    };

    expect(new SchemaRef(graph).toJsonSchema()).toEqual({
      $schema: 'https://json-schema.org/draft/2020-12/schema',
      type: 'array',
      prefixItems: [{ $ref: '#/$defs/person' }, { type: 'integer', minimum: 0, maximum: 255 }],
      items: false,
      minItems: 2,
      $defs: {
        person: {
          title: 'Person',
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Display name', examples: ['Ada'] },
            nickname: { oneOf: [{ type: 'null' }, { type: 'string' }] },
          },
          required: ['name'],
          additionalProperties: false,
        },
      },
    });
  });

  it('renders the same canonical shapes used by rich JSON values', () => {
    const root = t.record([
      field('text', schemaType({ tag: 'text', restrictions: {} })),
      field('duration', t.duration()),
      field('quantity', t.quantity({ baseUnit: 'm', allowedUnits: [] })),
    ]);

    expect(schema(root).toJsonSchema()).toMatchObject({
      properties: {
        text: {
          type: 'object',
          properties: { text: { type: 'string' }, language: { type: 'string' } },
          required: ['text'],
          additionalProperties: false,
        },
        duration: { type: 'string', format: 'duration' },
        quantity: {
          type: 'object',
          properties: { mantissa: { type: 'integer' } },
        },
      },
    });
  });
});
