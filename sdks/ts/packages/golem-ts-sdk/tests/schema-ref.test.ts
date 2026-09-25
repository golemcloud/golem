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
  it('decodes omitted and explicit-null option fields as absent', () => {
    const ref = schema(t.record([field('maybe', t.option(t.string()))]));
    expect(ref.packJson({})).toEqual(v.record([v.option()]));
    expect(ref.packJson({ maybe: null })).toEqual(v.record([v.option()]));

    const inheritedName = schema(t.record([field('constructor', t.option(t.string()))]));
    expect(inheritedName.packJson({})).toEqual(v.record([v.option()]));
  });

  it('rejects finite JSON numbers that overflow f32 after narrowing', () => {
    expect(schema(t.f32()).validateJson(1e100).success).toBe(false);
    expect(() => schema(t.f32()).unpackJson(v.f32(Infinity))).toThrow(/finite JSON number/);
    expect(() => schema(t.f64()).unpackJson(v.f64(-Infinity))).toThrow(/finite JSON number/);
  });

  it('rejects native values outside declared restrictions before unpacking', () => {
    const ref = schema(t.u32({ min: { tag: 'unsigned', val: 10n } }));
    expect(() => ref.unpackJson(v.u32(1))).toThrow(/does not conform/);
  });

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
    expect(() =>
      schema(schemaType({ tag: 'binary', restrictions: {} })).unpackJson(
        v.binary(Uint8Array.from([1]), 'not a mime'),
      ),
    ).toThrow(/invalid MIME type/);
  });

  it('uses a canonical signed decimal nanosecond string for durations', () => {
    const ref = schema(t.duration());

    expect(ref.packJson({ nanoseconds: '62003000000' })).toEqual(v.duration(62_003_000_000n));
    expect(ref.unpackJson(v.duration(-90_000_000_000n))).toEqual({
      nanoseconds: '-90000000000',
    });
    expect(ref.validateJson({ nanoseconds: '-0' }).success).toBe(false);
    expect(ref.validateJson({ nanoseconds: '01' }).success).toBe(false);
  });

  it('uses canonical signed decimal strings for quantity mantissas', () => {
    const ref = schema(t.quantity({ baseUnit: 'm', allowedSuffixes: [] }));
    const json = { mantissa: '123', scale: -2, unit: 'm' } as const;

    expect(ref.packJson(json)).toEqual(v.quantity({ mantissa: 123n, scale: -2, unit: 'm' }));
    expect(ref.unpackJson(ref.packJson(json))).toEqual(json);
    expect(ref.validateJson({ ...json, mantissa: 123 }).success).toBe(false);
  });

  it('uses full-range canonical decimal strings for s64 and u64 values', () => {
    const signed = schema(t.s64());
    const unsigned = schema(t.u64());

    expect(signed.packJson('-9223372036854775808')).toEqual(v.s64(-(2n ** 63n)));
    expect(unsigned.packJson('18446744073709551615')).toEqual(v.u64(2n ** 64n - 1n));
    expect(signed.unpackJson(v.s64(2n ** 63n - 1n))).toBe('9223372036854775807');
    expect(unsigned.validateJson('-1').success).toBe(false);
    expect(signed.validateJson('9223372036854775808').success).toBe(false);
    expect(unsigned.validateJson('01').success).toBe(false);
  });

  it('rejects out-of-range and malformed primitive values', () => {
    expect(schema(t.s8()).validateJson(-129).success).toBe(false);
    expect(schema(t.u8()).validateJson(256).success).toBe(false);
    expect(schema(t.u32()).validateJson(1.5).success).toBe(false);
    expect(schema(t.string()).validateValue(v.bool(true)).success).toBe(false);
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
      field('quantity', t.quantity({ baseUnit: 'm', allowedSuffixes: [] })),
    ]);

    expect(schema(root).toJsonSchema()).toMatchObject({
      properties: {
        text: {
          type: 'object',
          properties: { text: { type: 'string' }, language: { type: 'string' } },
          required: ['text'],
          additionalProperties: false,
        },
        duration: {
          type: 'object',
          properties: {
            nanoseconds: {
              type: 'string',
              format: 'int64',
              pattern: '^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$',
              'x-golem-minimum': '-9223372036854775808',
              'x-golem-maximum': '9223372036854775807',
            },
          },
        },
        quantity: {
          type: 'object',
          properties: {
            mantissa: {
              type: 'string',
              format: 'int64',
              pattern: '^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$',
            },
          },
        },
      },
    });
  });

  it('renders full-range metadata for wide integers', () => {
    expect(schema(t.s64()).toJsonSchema()).toMatchObject({
      type: 'string',
      format: 'int64',
      pattern: '^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$',
      'x-golem-minimum': '-9223372036854775808',
      'x-golem-maximum': '9223372036854775807',
    });
    expect(schema(t.u64()).toJsonSchema()).toMatchObject({
      type: 'string',
      format: 'uint64',
      pattern: '^(?:0|[1-9][0-9]*)$',
      'x-golem-minimum': '0',
      'x-golem-maximum': '18446744073709551615',
    });
  });

  it('renders declared bounds for every narrow integer and float family', () => {
    for (const type of [
      t.s8({ min: { tag: 'signed', val: -12n }, max: { tag: 'signed', val: 12n } }),
      t.s16({ min: { tag: 'signed', val: -12n }, max: { tag: 'signed', val: 12n } }),
      t.s32({ min: { tag: 'signed', val: -12n }, max: { tag: 'signed', val: 12n } }),
      t.u8({ min: { tag: 'unsigned', val: 2n }, max: { tag: 'unsigned', val: 12n } }),
      t.u16({ min: { tag: 'unsigned', val: 2n }, max: { tag: 'unsigned', val: 12n } }),
      t.u32({ min: { tag: 'unsigned', val: 2n }, max: { tag: 'unsigned', val: 12n } }),
    ]) {
      expect(schema(type).toJsonSchema()).toMatchObject({
        minimum: type.body.tag.startsWith('s') ? -12 : 2,
        maximum: 12,
      });
    }

    for (const type of [
      t.f32({
        min: { tag: 'float-bits', val: 0xbff0000000000000n },
        max: { tag: 'float-bits', val: 0x3fe0000000000000n },
      }),
      t.f64({
        min: { tag: 'float-bits', val: 0xbff0000000000000n },
        max: { tag: 'float-bits', val: 0x3fe0000000000000n },
      }),
    ]) {
      expect(schema(type).toJsonSchema()).toMatchObject({ minimum: -1, maximum: 0.5 });
    }
  });

  it('renders enforceable rich-value allowlists and canonical base64url bytes', () => {
    const rendered = schema(
      t.record([
        field('text', schemaType({ tag: 'text', restrictions: { languages: ['en', 'de'] } })),
        field('binary', schemaType({ tag: 'binary', restrictions: { mimeTypes: ['image/png'] } })),
      ]),
    ).toJsonSchema();
    expect(rendered).toMatchObject({
      properties: {
        text: { properties: { language: { enum: ['en', 'de'] } } },
        binary: { properties: { mimeType: { enum: ['image/png'] } } },
      },
    });
    const pattern = (
      rendered as {
        properties: { binary: { properties: { bytes: { pattern: string } } } };
      }
    ).properties.binary.properties.bytes.pattern;
    expect(pattern).toBe(
      '^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$',
    );
    const regex = new RegExp(pattern, 'u');
    for (const valid of ['', 'AQ', 'AQI', 'AQID', '-_8']) expect(regex.test(valid)).toBe(true);
    for (const invalid of ['+/8', 'AQ==', '-_9', 'A']) expect(regex.test(invalid)).toBe(false);
  });

  it('makes reflection-only unsupported leaves unsatisfiable', () => {
    for (const type of [
      t.secret(t.string()),
      t.quotaToken({}),
      t.permissionCard({ polymorphic: false }),
      schemaType({ tag: 'future', element: t.string() }),
      t.stream(t.string()),
    ]) {
      expect(schema(type).toJsonSchema()).toMatchObject({ not: {} });
    }
  });
});
