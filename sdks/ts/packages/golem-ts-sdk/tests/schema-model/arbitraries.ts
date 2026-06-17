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

// fast-check arbitraries for the recursive schema model. These intentionally
// stress every `SchemaTypeBody` / `SchemaValue` case, including rich semantic
// nodes, discriminated unions, capability nodes, and (for graphs) references
// that can form recursive and mutually-recursive shapes.

import fc, { type Arbitrary } from 'fast-check';

import type {
  DiscriminatorRule,
  MetadataEnvelope,
  QuantityValue,
  Role,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaTypeDef,
  SchemaValue,
  TypedSchemaValue,
} from '../../src/internal/schema-model';

// --- s64 / u64 ranges ---
const S64_MIN = -(2n ** 63n);
const S64_MAX = 2n ** 63n - 1n;
const U64_MAX = 2n ** 64n - 1n;

const arbS64 = fc.bigInt({ min: S64_MIN, max: S64_MAX });
const arbU64 = fc.bigInt({ min: 0n, max: U64_MAX });

const arbName = fc.string({ maxLength: 8 });
const arbTypeId = fc.string({ minLength: 1, maxLength: 6 });

// --- metadata ---

const arbRole: Arbitrary<Role> = fc.oneof(
  fc.constant<Role>({ tag: 'multimodal' }),
  fc.record({ tag: fc.constant<'other'>('other'), val: fc.string({ maxLength: 8 }) }),
);

export const arbMetadata: Arbitrary<MetadataEnvelope> = fc.record({
  doc: fc.option(fc.string({ maxLength: 12 }), { nil: undefined }),
  aliases: fc.array(fc.string({ maxLength: 6 }), { maxLength: 3 }),
  examples: fc.array(fc.string({ maxLength: 6 }), { maxLength: 3 }),
  deprecated: fc.option(fc.string({ maxLength: 8 }), { nil: undefined }),
  role: fc.option(arbRole, { nil: undefined }),
});

// --- rich-type substructures ---

const arbTextRestrictions = fc.record({
  languages: fc.option(fc.array(fc.string({ maxLength: 5 }), { maxLength: 3 }), { nil: undefined }),
  minLength: fc.option(fc.nat({ max: 1000 }), { nil: undefined }),
  maxLength: fc.option(fc.nat({ max: 1000 }), { nil: undefined }),
  regex: fc.option(fc.string({ maxLength: 8 }), { nil: undefined }),
});

const arbBinaryRestrictions = fc.record({
  mimeTypes: fc.option(fc.array(fc.string({ maxLength: 8 }), { maxLength: 3 }), { nil: undefined }),
  minBytes: fc.option(fc.nat({ max: 1000 }), { nil: undefined }),
  maxBytes: fc.option(fc.nat({ max: 1000 }), { nil: undefined }),
});

const arbPathSpec = fc.record({
  direction: fc.constantFrom('input', 'output', 'in-out') as Arbitrary<
    'input' | 'output' | 'in-out'
  >,
  kind: fc.constantFrom('file', 'directory', 'any') as Arbitrary<'file' | 'directory' | 'any'>,
  allowedMimeTypes: fc.option(fc.array(fc.string({ maxLength: 8 }), { maxLength: 3 }), {
    nil: undefined,
  }),
  allowedExtensions: fc.option(fc.array(fc.string({ maxLength: 5 }), { maxLength: 3 }), {
    nil: undefined,
  }),
});

const arbUrlRestrictions = fc.record({
  allowedSchemes: fc.option(fc.array(fc.string({ maxLength: 6 }), { maxLength: 3 }), {
    nil: undefined,
  }),
  allowedHosts: fc.option(fc.array(fc.string({ maxLength: 8 }), { maxLength: 3 }), {
    nil: undefined,
  }),
});

const arbQuantityValue: Arbitrary<QuantityValue> = fc.record({
  mantissa: arbS64,
  scale: fc.integer({ min: -1000, max: 1000 }),
  unit: fc.string({ maxLength: 4 }),
});

const arbQuantitySpec = fc.record({
  baseUnit: fc.string({ maxLength: 4 }),
  allowedSuffixes: fc.array(fc.string({ maxLength: 4 }), { maxLength: 4 }),
  min: fc.option(arbQuantityValue, { nil: undefined }),
  max: fc.option(arbQuantityValue, { nil: undefined }),
});

const arbDiscriminator: Arbitrary<DiscriminatorRule> = fc.oneof(
  fc.record({ tag: fc.constant<'prefix'>('prefix'), val: fc.string({ maxLength: 6 }) }),
  fc.record({ tag: fc.constant<'suffix'>('suffix'), val: fc.string({ maxLength: 6 }) }),
  fc.record({ tag: fc.constant<'contains'>('contains'), val: fc.string({ maxLength: 6 }) }),
  fc.record({ tag: fc.constant<'regex'>('regex'), val: fc.string({ maxLength: 6 }) }),
  fc.record({
    tag: fc.constant<'field-equals'>('field-equals'),
    val: fc.record({
      fieldName: fc.string({ maxLength: 6 }),
      literal: fc.option(fc.string({ maxLength: 6 }), { nil: undefined }),
    }),
  }),
  fc.record({ tag: fc.constant<'field-absent'>('field-absent'), val: fc.string({ maxLength: 6 }) }),
);

// ============================================================
// Schema types
// ============================================================

const PRIM_TAGS = [
  'bool',
  's8',
  's16',
  's32',
  's64',
  'u8',
  'u16',
  'u32',
  'u64',
  'f32',
  'f64',
  'char',
  'string',
] as const;

function leafBodyArbs(ids: string[]): Arbitrary<SchemaTypeBody>[] {
  const arbs: Arbitrary<SchemaTypeBody>[] = [
    ...PRIM_TAGS.map((tag) => fc.constant({ tag } as SchemaTypeBody)),
    fc.record({ tag: fc.constant<'enum'>('enum'), cases: fc.array(arbName, { maxLength: 4 }) }),
    fc.record({ tag: fc.constant<'flags'>('flags'), names: fc.array(arbName, { maxLength: 4 }) }),
    fc.record({ tag: fc.constant<'text'>('text'), restrictions: arbTextRestrictions }),
    fc.record({ tag: fc.constant<'binary'>('binary'), restrictions: arbBinaryRestrictions }),
    fc.record({ tag: fc.constant<'path'>('path'), spec: arbPathSpec }),
    fc.record({ tag: fc.constant<'url'>('url'), restrictions: arbUrlRestrictions }),
    fc.constant({ tag: 'datetime' } as SchemaTypeBody),
    fc.constant({ tag: 'duration' } as SchemaTypeBody),
    fc.record({ tag: fc.constant<'quantity'>('quantity'), spec: arbQuantitySpec }),
    fc.record({
      tag: fc.constant<'secret'>('secret'),
      spec: fc.record({ category: fc.option(fc.string({ maxLength: 6 }), { nil: undefined }) }),
    }),
    fc.record({
      tag: fc.constant<'quota-token'>('quota-token'),
      spec: fc.record({ resourceName: fc.option(fc.string({ maxLength: 6 }), { nil: undefined }) }),
    }),
  ];
  if (ids.length > 0) {
    arbs.push(fc.constantFrom(...ids).map((id) => ({ tag: 'ref', id }) as SchemaTypeBody));
  }
  return arbs;
}

function schemaTypeArb(depth: number, ids: string[]): Arbitrary<SchemaType> {
  return fc.record({ body: bodyArb(depth, ids), metadata: arbMetadata });
}

function bodyArb(depth: number, ids: string[]): Arbitrary<SchemaTypeBody> {
  const leaves = leafBodyArbs(ids);
  if (depth <= 0) {
    return fc.oneof(...leaves);
  }
  const sub = (): Arbitrary<SchemaType> => schemaTypeArb(depth - 1, ids);
  const composites: Arbitrary<SchemaTypeBody>[] = [
    fc.record({
      tag: fc.constant<'record'>('record'),
      fields: fc.array(fc.record({ name: arbName, body: sub(), metadata: arbMetadata }), {
        maxLength: 4,
      }),
    }),
    fc.record({
      tag: fc.constant<'variant'>('variant'),
      cases: fc.array(
        fc.record({
          name: arbName,
          payload: fc.option(sub(), { nil: undefined }),
          metadata: arbMetadata,
        }),
        { maxLength: 4 },
      ),
    }),
    fc.record({ tag: fc.constant<'tuple'>('tuple'), elements: fc.array(sub(), { maxLength: 4 }) }),
    fc.record({ tag: fc.constant<'list'>('list'), element: sub() }),
    fc.record({
      tag: fc.constant<'fixed-list'>('fixed-list'),
      element: sub(),
      length: fc.nat({ max: 16 }),
    }),
    fc.record({ tag: fc.constant<'map'>('map'), key: sub(), value: sub() }),
    fc.record({ tag: fc.constant<'option'>('option'), element: sub() }),
    fc.record({
      tag: fc.constant<'result'>('result'),
      ok: fc.option(sub(), { nil: undefined }),
      err: fc.option(sub(), { nil: undefined }),
    }),
    fc.record({
      tag: fc.constant<'union'>('union'),
      branches: fc.array(
        fc.record({
          tag: arbName,
          body: sub(),
          discriminator: arbDiscriminator,
          metadata: arbMetadata,
        }),
        { maxLength: 3 },
      ),
    }),
    fc.record({
      tag: fc.constant<'future'>('future'),
      element: fc.option(sub(), { nil: undefined }),
    }),
    fc.record({
      tag: fc.constant<'stream'>('stream'),
      element: fc.option(sub(), { nil: undefined }),
    }),
  ];
  return fc.oneof(...leaves, ...composites);
}

/** Anonymous schema type (no `ref`s), depth-bounded. */
export const arbSchemaType: Arbitrary<SchemaType> = schemaTypeArb(3, []);

/** A self-contained schema graph whose defs/root may reference each other. */
export const arbSchemaGraph: Arbitrary<SchemaGraph> = fc
  .uniqueArray(arbTypeId, { maxLength: 4 })
  .chain((ids) => {
    const defBodyArb = (): Arbitrary<SchemaTypeDef> =>
      fc.record({
        name: fc.option(fc.string({ maxLength: 6 }), { nil: undefined }),
        body: schemaTypeArb(2, ids),
      });
    const defsArb: Arbitrary<SchemaTypeDef[]> =
      ids.length === 0 ? fc.constant([]) : fc.tuple(...ids.map(() => defBodyArb()));
    return fc
      .record({ defsList: defsArb, root: schemaTypeArb(2, ids) })
      .map(({ defsList, root }) => {
        const defs = new Map<string, SchemaTypeDef>();
        ids.forEach((id, i) => defs.set(id, defsList[i]));
        return { defs, root };
      });
  });

// ============================================================
// Schema values
// ============================================================

const arbDatetime = fc.record({
  seconds: arbS64,
  nanoseconds: fc.integer({ min: 0, max: 999_999_999 }),
});

const arbQuotaTokenValue = fc.record({
  environmentId: fc.record({ uuid: fc.record({ highBits: arbU64, lowBits: arbU64 }) }),
  resourceName: fc.string({ maxLength: 8 }),
  expectedUse: arbU64,
  lastCredit: arbS64,
  lastCreditAt: arbDatetime,
});

function leafValueArbs(): Arbitrary<SchemaValue>[] {
  return [
    fc.record({ tag: fc.constant<'bool'>('bool'), value: fc.boolean() }),
    fc.record({ tag: fc.constant<'s8'>('s8'), value: fc.integer({ min: -128, max: 127 }) }),
    fc.record({ tag: fc.constant<'s16'>('s16'), value: fc.integer({ min: -32768, max: 32767 }) }),
    fc.record({
      tag: fc.constant<'s32'>('s32'),
      value: fc.integer({ min: -2147483648, max: 2147483647 }),
    }),
    fc.record({ tag: fc.constant<'s64'>('s64'), value: arbS64 }),
    fc.record({ tag: fc.constant<'u8'>('u8'), value: fc.integer({ min: 0, max: 255 }) }),
    fc.record({ tag: fc.constant<'u16'>('u16'), value: fc.integer({ min: 0, max: 65535 }) }),
    fc.record({ tag: fc.constant<'u32'>('u32'), value: fc.integer({ min: 0, max: 4294967295 }) }),
    fc.record({ tag: fc.constant<'u64'>('u64'), value: arbU64 }),
    fc.record({ tag: fc.constant<'f32'>('f32'), value: fc.double() }),
    fc.record({ tag: fc.constant<'f64'>('f64'), value: fc.double() }),
    fc.record({
      tag: fc.constant<'char'>('char'),
      value: fc.string({ minLength: 1, maxLength: 1 }),
    }),
    fc.record({ tag: fc.constant<'string'>('string'), value: fc.string({ maxLength: 12 }) }),
    fc.record({ tag: fc.constant<'enum'>('enum'), caseIndex: fc.nat({ max: 10 }) }),
    fc.record({
      tag: fc.constant<'flags'>('flags'),
      flags: fc.array(fc.boolean(), { maxLength: 6 }),
    }),
    fc.record({
      tag: fc.constant<'text'>('text'),
      text: fc.string({ maxLength: 12 }),
      language: fc.option(fc.string({ maxLength: 5 }), { nil: undefined }),
    }),
    fc.record({
      tag: fc.constant<'binary'>('binary'),
      bytes: fc.uint8Array({ maxLength: 16 }),
      mimeType: fc.option(fc.string({ maxLength: 8 }), { nil: undefined }),
    }),
    fc.record({ tag: fc.constant<'path'>('path'), value: fc.string({ maxLength: 12 }) }),
    fc.record({ tag: fc.constant<'url'>('url'), value: fc.string({ maxLength: 12 }) }),
    fc.record({ tag: fc.constant<'datetime'>('datetime'), value: arbDatetime }),
    fc.record({ tag: fc.constant<'duration'>('duration'), nanoseconds: arbS64 }),
    fc.record({ tag: fc.constant<'quantity'>('quantity'), value: arbQuantityValue }),
    fc.record({ tag: fc.constant<'secret'>('secret'), secretRef: fc.string({ maxLength: 10 }) }),
    fc.record({ tag: fc.constant<'quota-token'>('quota-token'), value: arbQuotaTokenValue }),
  ];
}

function valueArb(depth: number): Arbitrary<SchemaValue> {
  const leaves = leafValueArbs();
  if (depth <= 0) {
    return fc.oneof(...leaves);
  }
  const sub = (): Arbitrary<SchemaValue> => valueArb(depth - 1);
  const composites: Arbitrary<SchemaValue>[] = [
    fc.record({ tag: fc.constant<'record'>('record'), fields: fc.array(sub(), { maxLength: 4 }) }),
    fc.record({
      tag: fc.constant<'variant'>('variant'),
      caseIndex: fc.nat({ max: 10 }),
      payload: fc.option(sub(), { nil: undefined }),
    }),
    fc.record({ tag: fc.constant<'tuple'>('tuple'), elements: fc.array(sub(), { maxLength: 4 }) }),
    fc.record({ tag: fc.constant<'list'>('list'), elements: fc.array(sub(), { maxLength: 4 }) }),
    fc.record({
      tag: fc.constant<'fixed-list'>('fixed-list'),
      elements: fc.array(sub(), { maxLength: 4 }),
    }),
    fc.record({
      tag: fc.constant<'map'>('map'),
      entries: fc.array(fc.record({ key: sub(), value: sub() }), { maxLength: 4 }),
    }),
    fc.record({
      tag: fc.constant<'option'>('option'),
      value: fc.option(sub(), { nil: undefined }),
    }),
    fc.record({
      tag: fc.constant<'result'>('result'),
      result: fc.oneof(
        fc.record({
          tag: fc.constant<'ok'>('ok'),
          value: fc.option(sub(), { nil: undefined }),
        }),
        fc.record({
          tag: fc.constant<'err'>('err'),
          value: fc.option(sub(), { nil: undefined }),
        }),
      ),
    }),
    fc.record({
      tag: fc.constant<'union'>('union'),
      unionTag: arbName,
      body: sub(),
    }),
  ];
  return fc.oneof(...leaves, ...composites);
}

export const arbSchemaValue: Arbitrary<SchemaValue> = valueArb(3);

/**
 * A typed schema value pairing an arbitrary graph with an arbitrary value. The
 * two need not be schema-consistent: the codec roundtrip is structural and does
 * not validate the value against the schema.
 */
export const arbTypedSchemaValue: Arbitrary<TypedSchemaValue> = fc.record({
  graph: arbSchemaGraph,
  value: arbSchemaValue,
});
