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

// Parity checks for the compiled codecs. They must be byte-identical to the
// interpreted fused codec on encode and value-equal on decode for every
// supported shape — the agent-id hash and wire compatibility depend on it.

import { test, expect, describe } from 'vitest';
import {
  serializeGraphToWit,
  deserializeGraphFromWit,
  compileGraphEncoder,
  compileGraphDecoder,
} from '../src/internal/mapping/values/schemaValue';
import { r, resolvedField, ResolvedGraph, TypeId } from '../src/internal/mapping/types/resolvedType';
import { Result } from '../src/host/result';

interface Case {
  label: string;
  graph: ResolvedGraph;
  value: unknown;
}

const TREE_ID: TypeId = 'M.Tree';
const treeGraph: ResolvedGraph = {
  defs: new Map([
    [
      TREE_ID,
      r.record(
        [resolvedField('value', r.u32()), resolvedField('children', r.list(r.ref(TREE_ID)))],
        'Tree',
        'M',
      ),
    ],
  ]),
  root: r.ref(TREE_ID),
};

// A tagged union (WIT variant): { tag, [valueKey]: payload }.
const taggedVariant = r.variant(
  true,
  [
    { name: 'empty' },
    { name: 'num', payload: r.u32(), valueKey: 'value' },
    { name: 'text', payload: r.string(), valueKey: 'value' },
    { name: 'maybe', payload: r.option(r.u32(), 'undefined'), valueKey: 'value' },
  ],
  'Tagged',
  'M',
);

// A plain union (untagged): the value itself is the payload, matched by shape.
const plainUnion = r.variant(
  false,
  [
    { name: 'none' },
    { name: 'num', payload: r.u32() },
    { name: 'text', payload: r.string() },
  ],
  'Plain',
  'M',
);

const cases: Case[] = [
  { label: 'u8 typed list', graph: { defs: new Map(), root: r.list(r.u8(), 'u8') }, value: new Uint8Array([1, 2, 3, 0, 255]) },
  { label: 'u32 number list', graph: { defs: new Map(), root: r.list(r.u32()) }, value: [0, 1, 2, 65535] },
  { label: 's64 bigint list', graph: { defs: new Map(), root: r.list(r.s64()) }, value: [1n, -2n, 3n] },
  { label: 'string list', graph: { defs: new Map(), root: r.list(r.string()) }, value: ['a', 'bb', ''] },
  { label: 'bool list', graph: { defs: new Map(), root: r.list(r.bool()) }, value: [true, false, true] },
  {
    label: 'enum list',
    graph: { defs: new Map(), root: r.list(r.enum(['red', 'green', 'blue'], 'Color', 'M')) },
    value: ['red', 'blue', 'green'],
  },
  {
    label: 'option list',
    graph: { defs: new Map(), root: r.list(r.option(r.u32(), 'undefined')) },
    value: [1, undefined, 3],
  },
  {
    label: 'tuple',
    graph: { defs: new Map(), root: r.tuple([r.u32(), r.string(), r.bool()]) },
    value: [7, 'hi', true],
  },
  {
    label: 'nested record',
    graph: {
      defs: new Map(),
      root: r.record(
        [
          resolvedField('name', r.string()),
          resolvedField(
            'items',
            r.list(r.record([resolvedField('id', r.u32()), resolvedField('label', r.string())], 'Item', 'M')),
          ),
        ],
        'Container',
        'M',
      ),
    },
    value: { name: 'c', items: [{ id: 1, label: 'a' }, { id: 2, label: 'b' }] },
  },
  {
    label: 'recursive tree',
    graph: treeGraph,
    value: { value: 1, children: [{ value: 2, children: [] }, { value: 3, children: [{ value: 4, children: [] }] }] },
  },

  // map
  {
    label: 'map<string, u32>',
    graph: { defs: new Map(), root: r.map(r.string(), r.u32()) },
    value: new Map<string, number>([['a', 1], ['b', 2], ['c', 3]]),
  },
  {
    label: 'map<u32, list<string>>',
    graph: { defs: new Map(), root: r.map(r.u32(), r.list(r.string())) },
    value: new Map<number, string[]>([[1, ['x', 'y']], [2, []]]),
  },
  {
    label: 'list of maps',
    graph: { defs: new Map(), root: r.list(r.map(r.string(), r.u32())) },
    value: [new Map([['a', 1]]), new Map([['b', 2], ['c', 3]])],
  },

  // tagged variant
  { label: 'tagged variant: empty', graph: { defs: new Map(), root: taggedVariant }, value: { tag: 'empty' } },
  { label: 'tagged variant: num', graph: { defs: new Map(), root: taggedVariant }, value: { tag: 'num', value: 42 } },
  { label: 'tagged variant: text', graph: { defs: new Map(), root: taggedVariant }, value: { tag: 'text', value: 'hi' } },
  {
    label: 'tagged variant: option payload (some)',
    graph: { defs: new Map(), root: taggedVariant },
    value: { tag: 'maybe', value: 5 },
  },
  {
    label: 'tagged variant: option payload (undefined)',
    graph: { defs: new Map(), root: taggedVariant },
    value: { tag: 'maybe', value: undefined },
  },
  {
    label: 'list of tagged variants',
    graph: { defs: new Map(), root: r.list(taggedVariant) },
    value: [{ tag: 'empty' }, { tag: 'num', value: 1 }, { tag: 'text', value: 'z' }],
  },

  // plain union
  { label: 'plain union: name', graph: { defs: new Map(), root: plainUnion }, value: 'none' },
  { label: 'plain union: num', graph: { defs: new Map(), root: plainUnion }, value: 42 },
  { label: 'plain union: text', graph: { defs: new Map(), root: plainUnion }, value: 'hello' },

  // result (inbuilt)
  {
    label: 'result<u32, string> inbuilt: ok',
    graph: { defs: new Map(), root: r.result(r.u32(), r.string(), { tag: 'inbuilt' }) },
    value: Result.ok(7),
  },
  {
    label: 'result<u32, string> inbuilt: err',
    graph: { defs: new Map(), root: r.result(r.u32(), r.string(), { tag: 'inbuilt' }) },
    value: Result.err('boom'),
  },
  {
    label: 'result<_, _> inbuilt absent: ok',
    graph: {
      defs: new Map(),
      root: r.result(undefined, undefined, { tag: 'inbuilt', okAbsent: 'undefined', errAbsent: 'undefined' }),
    },
    value: Result.ok(undefined),
  },
  {
    label: 'result<_, _> inbuilt absent: err',
    graph: {
      defs: new Map(),
      root: r.result(undefined, undefined, { tag: 'inbuilt', okAbsent: 'undefined', errAbsent: 'undefined' }),
    },
    value: Result.err(undefined),
  },

  // result (custom tagged record)
  {
    label: 'result custom double: ok',
    graph: {
      defs: new Map(),
      root: r.result(r.u32(), r.string(), { tag: 'custom', okValueName: 'value', errValueName: 'error' }),
    },
    value: { tag: 'ok', value: 7 },
  },
  {
    label: 'result custom double: err',
    graph: {
      defs: new Map(),
      root: r.result(r.u32(), r.string(), { tag: 'custom', okValueName: 'value', errValueName: 'error' }),
    },
    value: { tag: 'err', error: 'boom' },
  },

  // composite: record holding a map, a variant, and a result
  {
    label: 'record of map + variant + result',
    graph: {
      defs: new Map(),
      root: r.record(
        [
          resolvedField('counts', r.map(r.string(), r.u32())),
          resolvedField('choice', taggedVariant),
          resolvedField('outcome', r.result(r.u32(), r.string(), { tag: 'inbuilt' })),
        ],
        'Composite',
        'M',
      ),
    },
    value: {
      counts: new Map([['a', 1], ['b', 2]]),
      choice: { tag: 'num', value: 9 },
      outcome: Result.err('nope'),
    },
  },
];

describe('compiled codec parity', () => {
  for (const c of cases) {
    test(`${c.label}: encode byte-identical`, () => {
      const expected = serializeGraphToWit(c.value, c.graph);
      expect(compileGraphEncoder(c.graph)(c.value)).toEqual(expected);
    });

    test(`${c.label}: decode value-equal`, () => {
      const wit = serializeGraphToWit(c.value, c.graph);
      const expected = deserializeGraphFromWit(wit, c.graph);
      expect(compileGraphDecoder(c.graph)(wit)).toEqual(expected);
    });
  }
});
