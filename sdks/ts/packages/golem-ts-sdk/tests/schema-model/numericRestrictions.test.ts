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

// Numeric-restriction codec sync (issue #3532): the numeric `SchemaType`
// variants carry an inline `option<numeric-restrictions>`. These tests mirror
// the Rust cross-language golden vectors
// (`golem-common/src/schema/tests/mod.rs::golden_numeric_schema_types`) and the
// canonicalization invariants of `NumericRestrictions::normalize`.

import { describe, it, expect } from 'vitest';

import type { SchemaTypeBody as WitSchemaTypeBody } from 'golem:core/types@2.0.0';

import {
  schemaGraphToWit,
  schemaGraphFromWit,
  t,
  type NumericBound,
  type NumericRestrictions,
  type SchemaGraph,
  type SchemaType,
} from '../../src/internal/schema-model';

/** Canonical IEEE-754 `f64` bits for a float, as a `u64` (the `float-bits` payload). */
function f64Bits(x: number): bigint {
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, x, true);
  return view.getBigUint64(0, true);
}

const u = (v: bigint): NumericBound => ({ tag: 'unsigned', val: v });
const s = (v: bigint): NumericBound => ({ tag: 'signed', val: v });
const f = (v: number): NumericBound => ({ tag: 'float-bits', val: f64Bits(v) });

const I64_MAX = 2n ** 63n - 1n;
const U64_MAX = 2n ** 64n - 1n;

function graph(root: SchemaType): SchemaGraph {
  return { defs: new Map(), root };
}

/** A numeric body extracted back from the round-tripped graph root. */
function roundtripBody(root: SchemaType): SchemaType['body'] {
  return schemaGraphFromWit(schemaGraphToWit(graph(root))).root.body;
}

/** The encoded WIT body for the graph root (the wire shape). */
function encodedRootBody(root: SchemaType): WitSchemaTypeBody {
  const wit = schemaGraphToWit(graph(root));
  return wit.typeNodes[wit.root].body;
}

describe('numeric restrictions codec (golden vectors)', () => {
  // (label, model SchemaType, expected normalized restrictions on the model body)
  const cases: Array<[string, SchemaType, NumericRestrictions | undefined]> = [
    ['u32 bare', t.u32(), undefined],
    ['u32 min=1', t.u32({ min: u(1n) }), { min: u(1n) }],
    ['u32 min=1 +unit', t.u32({ min: u(1n), unit: 'items' }), { min: u(1n), unit: 'items' }],
    ['u32 bounds=(0,100)', t.u32({ min: u(0n), max: u(100n) }), { min: u(0n), max: u(100n) }],
    [
      'u32 bounds=(0,100) +unit',
      t.u32({ min: u(0n), max: u(100n), unit: 'percent' }),
      { min: u(0n), max: u(100n), unit: 'percent' },
    ],
    [
      's64 bounds=(0,i64::MAX)',
      t.s64({ min: s(0n), max: s(I64_MAX) }),
      { min: s(0n), max: s(I64_MAX) },
    ],
    [
      's64 bounds=(0,i64::MAX) +unit',
      t.s64({ min: s(0n), max: s(I64_MAX), unit: 'ns' }),
      { min: s(0n), max: s(I64_MAX), unit: 'ns' },
    ],
    [
      'u64 near u64::MAX',
      t.u64({ min: u(U64_MAX - 1n), max: u(U64_MAX) }),
      { min: u(U64_MAX - 1n), max: u(U64_MAX) },
    ],
    [
      'u64 near u64::MAX +unit',
      t.u64({ min: u(U64_MAX - 1n), max: u(U64_MAX), unit: 'bytes' }),
      { min: u(U64_MAX - 1n), max: u(U64_MAX), unit: 'bytes' },
    ],
    ['f64 min=0.0', t.f64({ min: f(0.0) }), { min: f(0.0) }],
    [
      'f64 min=0.0 +unit',
      t.f64({ min: f(0.0), unit: 'seconds' }),
      { min: f(0.0), unit: 'seconds' },
    ],
    ['s8 bounds=(-1,1)', t.s8({ min: s(-1n), max: s(1n) }), { min: s(-1n), max: s(1n) }],
    ['f32 max=1.5', t.f32({ max: f(1.5) }), { max: f(1.5) }],
  ];

  for (const [label, type, expected] of cases) {
    it(`round-trips ${label}`, () => {
      const back = roundtripBody(type);
      expect(back).toEqual({ tag: type.body.tag, restrictions: expected });
    });
  }
});

describe('numeric restrictions canonicalization', () => {
  it('encodes an unconstrained numeric as `none`', () => {
    expect(encodedRootBody(t.u32())).toEqual({ tag: 'u32-type', val: undefined });
  });

  it('collapses an empty restriction set to `none`', () => {
    expect(encodedRootBody(t.u32({}))).toEqual({ tag: 'u32-type', val: undefined });
    expect(roundtripBody(t.u32({}))).toEqual({ tag: 'u32', restrictions: undefined });
  });

  it('drops an empty `unit`', () => {
    expect(encodedRootBody(t.u32({ unit: '' }))).toEqual({ tag: 'u32-type', val: undefined });
    expect(roundtripBody(t.u32({ min: u(1n), unit: '' }))).toEqual({
      tag: 'u32',
      restrictions: { min: u(1n) },
    });
  });

  it('canonicalizes a `-0.0` float bound to `+0.0` bits', () => {
    const negZeroBits = f64Bits(-0.0);
    expect(negZeroBits).toBe(1n << 63n);
    const encoded = encodedRootBody(t.f64({ min: { tag: 'float-bits', val: negZeroBits } }));
    expect(encoded).toEqual({ tag: 'f64-type', val: { min: { tag: 'float-bits', val: 0n } } });
    expect(roundtripBody(t.f64({ min: { tag: 'float-bits', val: negZeroBits } }))).toEqual({
      tag: 'f64',
      restrictions: { min: { tag: 'float-bits', val: 0n } },
    });
  });

  it('preserves a constrained numeric through the wire shape', () => {
    expect(encodedRootBody(t.u32({ min: u(1n), max: u(100n), unit: 'percent' }))).toEqual({
      tag: 'u32-type',
      val: { min: u(1n), max: u(100n), unit: 'percent' },
    });
  });
});
