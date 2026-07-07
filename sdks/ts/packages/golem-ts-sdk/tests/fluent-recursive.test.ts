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

// Recursive-schema support: self- and mutually-recursive types (e.g. a `Tree`)
// must compile to a named `ref` def (not infinite-loop) and round-trip nested
// values, across all four vendor walkers. Non-recursive schemas must stay INLINE
// (no `ref`, no `def`) — the regression guard for existing behaviour.

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import * as vb from 'valibot';
import { scope } from 'arktype';
import { Schema } from 'effect';

import { compileSchema } from '../src/fluent/schema/adapter';
import '../src/fluent/schema/zod';
import '../src/fluent/schema/valibot';
import '../src/fluent/schema/arktype';
import '../src/fluent/schema/effect';

import {
  GraphEncoder,
  mergeGraphDefs,
  schemaGraphToWit,
  schemaGraphFromWit,
  schemaValueToWit,
  schemaValueFromWit,
  type SchemaType,
  type SchemaTypeBody,
} from '../src/internal/schema-model';
import type { FluentCodec } from '../src/fluent/schema/codec';

// A 3-level nested tree value shared by every vendor's round-trip test.
const sampleTree = {
  value: 1,
  children: [
    { value: 2, children: [] },
    { value: 3, children: [{ value: 4, children: [] }] },
  ],
};

/** The body of the record reached through a recursive codec's single `ref` def. */
function refDefBody(codec: FluentCodec): SchemaTypeBody {
  expect(codec.graph.root.body.tag).toBe('ref');
  expect(codec.graph.defs.size).toBe(1);
  const [def] = [...codec.graph.defs.values()];
  // `SchemaTypeDef.body` is a `SchemaType` (`{ body, metadata }`); its `.body` is
  // the structural `SchemaTypeBody`.
  return def.body.body;
}

/**
 * Prove the recursive graph encodes to the flat WIT carrier and back — exercising
 * the `ref-type` path the way `runtime.ts assembleAgentType` does (seed a
 * `GraphEncoder` with the merged defs, then encode the root), then decode the
 * whole graph to confirm the cyclic `ref` survives a full round-trip.
 */
function assertGraphEncodes(codec: FluentCodec): void {
  // assembleAgentType path: shared encoder seeded with the merged named defs.
  const encoder = new GraphEncoder(mergeGraphDefs([codec.graph]));
  const rootIdx = encoder.encodeType(codec.graph.root);
  const witGraph = encoder.finish();
  expect(typeof rootIdx).toBe('number');
  expect(witGraph.defs.length).toBe(1);

  // Full graph → WIT → graph round-trip (guards against cyclic-index errors).
  const back = schemaGraphFromWit(schemaGraphToWit(codec.graph));
  expect(back.root.body.tag).toBe('ref');
  expect(back.defs.size).toBe(1);
}

/** Prove a value round-trips both directly and through the flat WIT value tree. */
function assertValueRoundTrips(codec: FluentCodec, value: unknown): void {
  expect(codec.fromValue(codec.toValue(value))).toEqual(value);
  // Through the WIT value carrier (finite value tree → no value-node cycle).
  const wit = schemaValueToWit(codec.toValue(value));
  expect(codec.fromValue(schemaValueFromWit(wit))).toEqual(value);
}

// ============================================================
// Zod
// ============================================================

describe('fluent recursive schemas — Zod', () => {
  it('compiles a self-recursive Tree to a single ref def and round-trips a nested value', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const Tree: z.ZodType<any> = z.object({
      value: z.number(),
      children: z.array(z.lazy(() => Tree)),
    });

    const codec = compileSchema(Tree);

    // A single named def; the root is a `ref` to it (no infinite loop / throw).
    const body = refDefBody(codec);
    expect(body.tag).toBe('record');

    // The def's `children` field is `list<ref>` closing the recursion.
    const fields = (body as { tag: 'record'; fields: { name: string; body: SchemaType }[] }).fields;
    const children = fields.find((f) => f.name === 'children')!;
    expect(children.body.body.tag).toBe('list');
    const elem = (children.body.body as { tag: 'list'; element: SchemaType }).element;
    expect(elem.body.tag).toBe('ref');

    assertValueRoundTrips(codec, sampleTree);
    assertGraphEncodes(codec);
  });

  it('keeps a NON-recursive object INLINE (no ref, empty defs)', () => {
    const codec = compileSchema(z.object({ a: z.number(), b: z.string() }));
    expect(codec.graph.root.body.tag).toBe('record');
    expect(codec.graph.defs.size).toBe(0);
  });

  it('round-trips a mutually-recursive pair (A has B, B has A)', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const A: z.ZodType<any> = z.object({
      name: z.string(),
      bs: z.array(z.lazy(() => B)),
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const B: z.ZodType<any> = z.object({
      tag: z.number(),
      as: z.array(z.lazy(() => A)),
    });

    const codec = compileSchema(A);
    expect(codec.graph.root.body.tag).toBe('ref');

    const value = {
      name: 'root',
      bs: [
        { tag: 1, as: [{ name: 'leaf', bs: [] }] },
        { tag: 2, as: [] },
      ],
    };
    assertValueRoundTrips(codec, value);
    assertGraphEncodes(codec);
  });
});

// ============================================================
// Valibot
// ============================================================

describe('fluent recursive schemas — Valibot', () => {
  it('compiles a self-recursive Tree and round-trips a nested value', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const Tree: any = vb.object({
      value: vb.number(),
      children: vb.array(vb.lazy(() => Tree)),
    });

    const codec = compileSchema(Tree);
    expect(refDefBody(codec).tag).toBe('record');
    assertValueRoundTrips(codec, sampleTree);
    assertGraphEncodes(codec);
  });

  it('keeps a NON-recursive object INLINE', () => {
    const codec = compileSchema(vb.object({ a: vb.number(), b: vb.string() }));
    expect(codec.graph.root.body.tag).toBe('record');
    expect(codec.graph.defs.size).toBe(0);
  });
});

// ============================================================
// Effect Schema
// ============================================================

// Object-form Standard Schema facade (mirrors tests/fluent-vendors.test.ts).
const std = <A, I>(schema: Schema.Schema<A, I>) => {
  const fnForm = Schema.standardSchemaV1(schema);
  return { '~standard': fnForm['~standard'], ast: schema.ast };
};

describe('fluent recursive schemas — Effect Schema', () => {
  it('compiles a Schema.suspend-recursive Tree and round-trips a nested value', () => {
    interface ETree {
      readonly value: number;
      readonly children: ReadonlyArray<ETree>;
    }
    const ETree: Schema.Schema<ETree> = Schema.Struct({
      value: Schema.Number,
      children: Schema.Array(Schema.suspend((): Schema.Schema<ETree> => ETree)),
    });

    const codec = compileSchema(std(ETree));
    expect(refDefBody(codec).tag).toBe('record');
    assertValueRoundTrips(codec, sampleTree);
    assertGraphEncodes(codec);
  });

  it('keeps a NON-recursive struct INLINE', () => {
    const codec = compileSchema(std(Schema.Struct({ a: Schema.Number, b: Schema.String })));
    expect(codec.graph.root.body.tag).toBe('record');
    expect(codec.graph.defs.size).toBe(0);
  });
});

// ============================================================
// ArkType
// ============================================================

describe('fluent recursive schemas — ArkType', () => {
  it('compiles a scope-recursive Tree (alias node) and round-trips a nested value', () => {
    const types = scope({
      tree: { value: 'number', 'children?': 'tree[]' },
    }).export();
    const Tree = types.tree as unknown as { '~standard': unknown; internal: unknown };
    const facade = { '~standard': Tree['~standard'], internal: Tree.internal };

    const codec = compileSchema(facade);
    expect(refDefBody(codec).tag).toBe('record');

    // ArkType marks `children` optional (`children?`), so it round-trips as
    // `option<list<ref>>`; supply the field explicitly at each level.
    const arkTree = {
      value: 1,
      children: [
        { value: 2, children: [] },
        { value: 3, children: [{ value: 4, children: [] }] },
      ],
    };
    expect(codec.fromValue(codec.toValue(arkTree))).toEqual(arkTree);
    assertGraphEncodes(codec);
  });

  it('keeps a NON-recursive object INLINE', () => {
    const types = scope({ point: { x: 'number', y: 'number' } }).export();
    const Point = types.point as unknown as { '~standard': unknown; internal: unknown };
    const codec = compileSchema({ '~standard': Point['~standard'], internal: Point.internal });
    expect(codec.graph.root.body.tag).toBe('record');
    expect(codec.graph.defs.size).toBe(0);
  });
});
