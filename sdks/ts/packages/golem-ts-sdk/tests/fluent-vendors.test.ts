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

// Round-trip tests for the Valibot / ArkType / Effect Schema fluent walkers. Each
// vendor library is a devDependency imported here (in test code only — the walker
// modules themselves never import the libraries). Importing a walker module has
// the side effect of registering its `SchemaWalker` with the adapter.

import { describe, it, expect } from 'vitest';
import * as vb from 'valibot';
import { type as arkType } from 'arktype';
import { Schema } from 'effect';

import { compileSchema } from '../src/fluent/schema/adapter';
import '../src/fluent/schema/valibot';
import '../src/fluent/schema/arktype';
import '../src/fluent/schema/effect';

// ArkType's `type(...)` is itself a *callable function* (it carries `~standard`
// and `.internal`), but the adapter dispatches only on object Standard Schema
// values. The arktype entry point therefore exposes an object-form Standard
// Schema: a plain object carrying the same `~standard` props plus the `.internal`
// node tree the walker reads. We reproduce that thin facade here.
const ark = (def: unknown) => {
  const t = arkType(def as never) as unknown as {
    '~standard': unknown;
    internal: unknown;
  };
  return { '~standard': t['~standard'], internal: t.internal };
};

// ============================================================
// Valibot (vendor '~standard'.vendor === 'valibot')
// ============================================================

describe('fluent Valibot walker', () => {
  it('reports the expected ~standard vendor', () => {
    expect(vb.string()['~standard'].vendor).toBe('valibot');
  });

  it('maps primitive schemas and round-trips values', () => {
    const str = compileSchema(vb.string());
    expect(str.graph.root.body.tag).toBe('string');
    expect(str.fromValue(str.toValue('hi'))).toBe('hi');

    const num = compileSchema(vb.number());
    expect(num.graph.root.body.tag).toBe('f64');
    expect(num.fromValue(num.toValue(5))).toBe(5);

    expect(compileSchema(vb.boolean()).graph.root.body.tag).toBe('bool');
    expect(compileSchema(vb.bigint()).graph.root.body.tag).toBe('u64');

    const big = compileSchema(vb.bigint());
    expect(big.fromValue(big.toValue(42n))).toBe(42n);
  });

  it('maps optional/nullable to option', () => {
    const opt = compileSchema(vb.optional(vb.number()));
    expect(opt.graph.root.body.tag).toBe('option');
    expect(opt.fromValue(opt.toValue(3))).toBe(3);
    expect(opt.fromValue(opt.toValue(undefined))).toBeUndefined();

    const nul = compileSchema(vb.nullable(vb.string()));
    expect(nul.graph.root.body.tag).toBe('option');
    expect(nul.fromValue(nul.toValue('x'))).toBe('x');
  });

  it('maps arrays element-wise', () => {
    const arr = compileSchema(vb.array(vb.string()));
    expect(arr.graph.root.body.tag).toBe('list');
    expect(arr.fromValue(arr.toValue(['a', 'b']))).toEqual(['a', 'b']);
  });

  it('maps objects to records preserving field order', () => {
    const obj = compileSchema(vb.object({ x: vb.string(), y: vb.number() }));
    expect(obj.graph.root.body.tag).toBe('record');
    const fields = (obj.graph.root.body as { tag: 'record'; fields: { name: string }[] }).fields;
    expect(fields.map((f) => f.name)).toEqual(['x', 'y']);
    const value = { x: 'hi', y: 2 };
    expect(obj.fromValue(obj.toValue(value))).toEqual(value);
  });

  it('maps tuples element-wise', () => {
    const tup = compileSchema(vb.tuple([vb.string(), vb.number(), vb.boolean()]));
    expect(tup.graph.root.body.tag).toBe('tuple');
    const value = ['a', 1, true];
    expect(tup.fromValue(tup.toValue(value))).toEqual(value);
  });

  it('maps picklist to enum by case index', () => {
    const en = compileSchema(vb.picklist(['red', 'green', 'blue']));
    expect(en.graph.root.body).toMatchObject({ tag: 'enum', cases: ['red', 'green', 'blue'] });
    expect(en.toValue('green')).toEqual({ tag: 'enum', caseIndex: 1 });
    expect(en.fromValue({ tag: 'enum', caseIndex: 2 })).toBe('blue');
  });

  it('maps a literal to its base primitive', () => {
    const lit = compileSchema(vb.literal('ok'));
    expect(lit.graph.root.body.tag).toBe('string');
    expect(lit.fromValue(lit.toValue('ok'))).toBe('ok');
  });

  it('maps record to a map node', () => {
    const rec = compileSchema(vb.record(vb.string(), vb.number()));
    expect(rec.graph.root.body.tag).toBe('map');
    const value = { a: 1, b: 2 };
    expect(rec.fromValue(rec.toValue(value))).toEqual(value);
  });

  it('maps map to a map node (arbitrary keys)', () => {
    const m = compileSchema(vb.map(vb.string(), vb.number()));
    expect(m.graph.root.body.tag).toBe('map');
    const value = new Map([
      ['a', 1],
      ['b', 2],
    ]);
    expect(m.fromValue(m.toValue(value))).toEqual(value);
  });

  it('maps a discriminated variant to a WIT variant', () => {
    const variant = compileSchema(
      vb.variant('kind', [
        vb.object({ kind: vb.literal('a'), x: vb.string() }),
        vb.object({ kind: vb.literal('b'), y: vb.number() }),
      ]),
    );
    expect(variant.graph.root.body.tag).toBe('variant');
    const a = { kind: 'a', x: 'hi' };
    const b = { kind: 'b', y: 7 };
    expect(variant.toValue(a)).toMatchObject({ tag: 'variant', caseIndex: 0 });
    expect(variant.fromValue(variant.toValue(a))).toEqual(a);
    expect(variant.fromValue(variant.toValue(b))).toEqual(b);
  });

  it('maps a plain (non-discriminated) union to a variant and round-trips by case', () => {
    const u = compileSchema(vb.union([vb.string(), vb.number(), vb.boolean()]));
    expect(u.graph.root.body.tag).toBe('variant');
    expect(
      (u.graph.root.body as { tag: 'variant'; cases: { name: string }[] }).cases.map((c) => c.name),
    ).toEqual(['case0', 'case1', 'case2']);
    for (const [val, idx] of [['hi', 0] as const, [5, 1] as const, [true, 2] as const]) {
      expect((u.toValue(val) as { caseIndex: number }).caseIndex).toBe(idx);
      expect(u.fromValue(u.toValue(val))).toEqual(val);
    }
  });
});

// ============================================================
// ArkType (vendor '~standard'.vendor === 'arktype')
// ============================================================

describe('fluent ArkType walker', () => {
  it('reports the expected ~standard vendor', () => {
    expect(arkType('string')['~standard'].vendor).toBe('arktype');
  });

  it('maps primitive schemas and round-trips values', () => {
    const str = compileSchema(ark('string'));
    expect(str.graph.root.body.tag).toBe('string');
    expect(str.fromValue(str.toValue('hi'))).toBe('hi');

    const num = compileSchema(ark('number'));
    expect(num.graph.root.body.tag).toBe('f64');
    expect(num.fromValue(num.toValue(5))).toBe(5);

    const bool = compileSchema(ark('boolean'));
    expect(bool.graph.root.body.tag).toBe('bool');
    expect(bool.fromValue(bool.toValue(true))).toBe(true);

    const big = compileSchema(ark('bigint'));
    expect(big.graph.root.body.tag).toBe('u64');
    expect(big.fromValue(big.toValue(42n))).toBe(42n);
  });

  it('maps optional/nullable to option', () => {
    const opt = compileSchema(ark('string | undefined'));
    expect(opt.graph.root.body.tag).toBe('option');
    expect(opt.fromValue(opt.toValue('x'))).toBe('x');
    expect(opt.fromValue(opt.toValue(undefined))).toBeUndefined();

    const nul = compileSchema(ark('number | null'));
    expect(nul.graph.root.body.tag).toBe('option');
    expect(nul.fromValue(nul.toValue(3))).toBe(3);
  });

  it('maps arrays element-wise', () => {
    const arr = compileSchema(ark('string[]'));
    expect(arr.graph.root.body.tag).toBe('list');
    expect(arr.fromValue(arr.toValue(['a', 'b']))).toEqual(['a', 'b']);
  });

  it('maps objects to records and round-trips', () => {
    const obj = compileSchema(ark({ x: 'string', y: 'number' }));
    expect(obj.graph.root.body.tag).toBe('record');
    const fields = (obj.graph.root.body as { tag: 'record'; fields: { name: string }[] }).fields;
    // ArkType sorts required keys; x precedes y here regardless.
    expect(fields.map((f) => f.name).sort()).toEqual(['x', 'y']);
    const value = { x: 'hi', y: 2 };
    expect(obj.fromValue(obj.toValue(value))).toEqual(value);
  });

  it('maps an object with an optional property to a record with an option field', () => {
    const obj = compileSchema(ark({ a: 'string', 'b?': 'number' }));
    expect(obj.graph.root.body.tag).toBe('record');
    const full = { a: 'hi', b: 2 };
    expect(obj.fromValue(obj.toValue(full))).toEqual(full);
    const partial = { a: 'hi' };
    expect(obj.fromValue(obj.toValue(partial))).toEqual(partial);
  });

  it('maps tuples element-wise', () => {
    const tup = compileSchema(ark(['string', 'number', 'boolean']));
    expect(tup.graph.root.body.tag).toBe('tuple');
    const value = ['a', 1, true];
    expect(tup.fromValue(tup.toValue(value))).toEqual(value);
  });

  it('maps a string-literal union to an enum', () => {
    const en = compileSchema(ark("'red' | 'green' | 'blue'"));
    expect(en.graph.root.body.tag).toBe('enum');
    const cases = (en.graph.root.body as { tag: 'enum'; cases: string[] }).cases;
    expect(cases.sort()).toEqual(['blue', 'green', 'red']);
    // Round-trip each case through its own index.
    for (const c of cases) {
      expect(en.fromValue(en.toValue(c))).toBe(c);
    }
  });

  it('maps a single literal to its base primitive', () => {
    const lit = compileSchema(ark("'ok'"));
    expect(lit.graph.root.body.tag).toBe('string');
    expect(lit.fromValue(lit.toValue('ok'))).toBe('ok');
  });

  it('maps an index-signature object to a map node', () => {
    const rec = compileSchema(ark({ '[string]': 'number' }));
    expect(rec.graph.root.body.tag).toBe('map');
    const value = { a: 1, b: 2 };
    expect(rec.fromValue(rec.toValue(value))).toEqual(value);
  });

  it('maps a plain (non-literal) union to a variant and round-trips by structure', () => {
    const u = compileSchema(ark('string | number'));
    expect(u.graph.root.body.tag).toBe('variant');
    // ArkType may normalize branch order, so assert round-trip correctness
    // (structural disambiguation picks the right case) rather than a fixed index.
    expect(u.fromValue(u.toValue('hi'))).toBe('hi');
    expect(u.fromValue(u.toValue(5))).toBe(5);
  });
});

// ============================================================
// Effect Schema (vendor '~standard'.vendor === 'effect')
// ============================================================

// Effect's own `Schema.standardSchemaV1(...)` returns a *function* (it carries
// `~standard` + `.ast`), but the adapter dispatches only on object Standard
// Schema values. The effect entry point therefore exposes an object-form
// Standard Schema: a plain object carrying the same `~standard` props plus the
// schema `.ast` the walker reads. We reproduce that thin facade here.
const std = <A, I>(schema: Schema.Schema<A, I>) => {
  const fnForm = Schema.standardSchemaV1(schema);
  return { '~standard': fnForm['~standard'], ast: schema.ast };
};

describe('fluent Effect Schema walker', () => {
  it('reports the expected ~standard vendor', () => {
    expect(std(Schema.String)['~standard'].vendor).toBe('effect');
  });

  it('maps primitive schemas and round-trips values', () => {
    const str = compileSchema(std(Schema.String));
    expect(str.graph.root.body.tag).toBe('string');
    expect(str.fromValue(str.toValue('hi'))).toBe('hi');

    const num = compileSchema(std(Schema.Number));
    expect(num.graph.root.body.tag).toBe('f64');
    expect(num.fromValue(num.toValue(5))).toBe(5);

    expect(compileSchema(std(Schema.Boolean)).graph.root.body.tag).toBe('bool');

    const big = compileSchema(std(Schema.BigIntFromSelf));
    expect(big.graph.root.body.tag).toBe('u64');
    expect(big.fromValue(big.toValue(42n))).toBe(42n);
  });

  it('maps NullOr/UndefinedOr to option', () => {
    const opt = compileSchema(std(Schema.UndefinedOr(Schema.Number)));
    expect(opt.graph.root.body.tag).toBe('option');
    expect(opt.fromValue(opt.toValue(3))).toBe(3);
    expect(opt.fromValue(opt.toValue(undefined))).toBeUndefined();

    const nul = compileSchema(std(Schema.NullOr(Schema.String)));
    expect(nul.graph.root.body.tag).toBe('option');
    expect(nul.fromValue(nul.toValue('x'))).toBe('x');
  });

  it('maps arrays element-wise', () => {
    const arr = compileSchema(std(Schema.Array(Schema.String)));
    expect(arr.graph.root.body.tag).toBe('list');
    expect(arr.fromValue(arr.toValue(['a', 'b']))).toEqual(['a', 'b']);
  });

  it('maps structs to records preserving field order', () => {
    const obj = compileSchema(std(Schema.Struct({ x: Schema.String, y: Schema.Number })));
    expect(obj.graph.root.body.tag).toBe('record');
    const fields = (obj.graph.root.body as { tag: 'record'; fields: { name: string }[] }).fields;
    expect(fields.map((f) => f.name)).toEqual(['x', 'y']);
    const value = { x: 'hi', y: 2 };
    expect(obj.fromValue(obj.toValue(value))).toEqual(value);
  });

  it('maps an optional struct property to an option field', () => {
    const obj = compileSchema(
      std(Schema.Struct({ a: Schema.String, b: Schema.optional(Schema.Number) })),
    );
    expect(obj.graph.root.body.tag).toBe('record');
    expect(obj.fromValue(obj.toValue({ a: 'hi', b: 2 }))).toEqual({ a: 'hi', b: 2 });
    expect(obj.fromValue(obj.toValue({ a: 'hi' }))).toEqual({ a: 'hi' });
  });

  it('maps tuples element-wise', () => {
    const tup = compileSchema(std(Schema.Tuple(Schema.String, Schema.Number, Schema.Boolean)));
    expect(tup.graph.root.body.tag).toBe('tuple');
    const value = ['a', 1, true];
    expect(tup.fromValue(tup.toValue(value))).toEqual(value);
  });

  it('maps a string-literal union to an enum', () => {
    const en = compileSchema(std(Schema.Literal('red', 'green', 'blue')));
    expect(en.graph.root.body).toMatchObject({ tag: 'enum', cases: ['red', 'green', 'blue'] });
    expect(en.toValue('green')).toEqual({ tag: 'enum', caseIndex: 1 });
    expect(en.fromValue({ tag: 'enum', caseIndex: 2 })).toBe('blue');
  });

  it('maps a single literal to its base primitive', () => {
    const lit = compileSchema(std(Schema.Literal('ok')));
    expect(lit.graph.root.body.tag).toBe('string');
    expect(lit.fromValue(lit.toValue('ok'))).toBe('ok');
  });

  it('maps native Enums to an enum node', () => {
    enum Color {
      Red = 'red',
      Green = 'green',
    }
    const en = compileSchema(std(Schema.Enums(Color)));
    expect(en.graph.root.body.tag).toBe('enum');
    expect(en.fromValue(en.toValue(Color.Green))).toBe(Color.Green);
  });

  it('maps a Record to a map node', () => {
    const rec = compileSchema(std(Schema.Record({ key: Schema.String, value: Schema.Number })));
    expect(rec.graph.root.body.tag).toBe('map');
    const value = { a: 1, b: 2 };
    expect(rec.fromValue(rec.toValue(value))).toEqual(value);
  });

  it('maps a tagged union to a WIT variant', () => {
    const A = Schema.Struct({ _tag: Schema.Literal('a'), x: Schema.String });
    const B = Schema.Struct({ _tag: Schema.Literal('b'), y: Schema.Number });
    const variant = compileSchema(std(Schema.Union(A, B)));
    expect(variant.graph.root.body.tag).toBe('variant');
    const a = { _tag: 'a', x: 'hi' };
    const b = { _tag: 'b', y: 7 };
    expect(variant.toValue(a)).toMatchObject({ tag: 'variant', caseIndex: 0 });
    expect(variant.fromValue(variant.toValue(a))).toEqual(a);
    expect(variant.fromValue(variant.toValue(b))).toEqual(b);
  });

  it('maps a plain (non-tagged) union to a variant and round-trips by structure', () => {
    const u = compileSchema(std(Schema.Union(Schema.String, Schema.Number, Schema.Boolean)));
    expect(u.graph.root.body.tag).toBe('variant');
    for (const val of ['hi', 5, true] as const) {
      expect(u.fromValue(u.toValue(val))).toEqual(val);
    }
  });
});
