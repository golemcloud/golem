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

import { describe, it, expect } from 'vitest';

import {
  SchemaBuilder,
  SchemaConflictError,
  type SchemaGraph,
  type SchemaTypeDef,
  field,
  mergeAgentGraphs,
  mergeGraphDefs,
  schemaGraphFromWit,
  schemaGraphToWit,
  t,
} from '../../src/internal/schema-model';

function pointDef(): SchemaTypeDef {
  return { body: t.record([field('x', t.f64()), field('y', t.f64())]) };
}

describe('SchemaBuilder reserve/commit protocol', () => {
  it('contains reflects reservations and commits', () => {
    const b = new SchemaBuilder();
    expect(b.contains('a.T')).toBe(false);
    b.reserve('a.T');
    expect(b.contains('a.T')).toBe(true);
  });

  it('finish() throws if a reserved id was never committed', () => {
    const b = new SchemaBuilder();
    b.reserve('a.Unfinished');
    expect(() => b.buildGraph(b.ref('a.Unfinished'))).toThrow(/never committed/);
  });

  it('register is idempotent: the first body wins, both calls return the same ref', () => {
    const b = new SchemaBuilder();
    const first = b.register('a.Foo', () => t.record([field('x', t.s32())]));
    const second = b.register('a.Foo', () => t.record([field('y', t.string())]));
    expect(first).toEqual(second);
    const def = b.finish().get('a.Foo')!;
    expect(def.body).toEqual(t.record([field('x', t.s32())]));
  });

  it('register attaches an optional display name', () => {
    const b = new SchemaBuilder();
    b.register('a.Named', () => t.bool(), 'Human Name');
    const def = b.finish().get('a.Named')!;
    expect(def.name).toBe('Human Name');
  });
});

describe('mergeGraphDefs / mergeAgentGraphs', () => {
  it('dedups structurally-identical definitions sharing a type-id', () => {
    const g1: SchemaGraph = { defs: new Map([['g.Point', pointDef()]]), root: t.ref('g.Point') };
    const g2: SchemaGraph = {
      defs: new Map([['g.Point', pointDef()]]),
      root: t.list(t.ref('g.Point')),
    };

    const defs = mergeGraphDefs([g1, g2]);
    expect([...defs.keys()]).toEqual(['g.Point']);
  });

  it('mergeAgentGraphs keeps all roots in input order with a shared def registry', () => {
    const g1: SchemaGraph = { defs: new Map([['g.Point', pointDef()]]), root: t.ref('g.Point') };
    const g2: SchemaGraph = {
      defs: new Map([['g.Point', pointDef()]]),
      root: t.option(t.ref('g.Point')),
    };

    const merged = mergeAgentGraphs([g1, g2]);
    expect([...merged.defs.keys()]).toEqual(['g.Point']);
    expect(merged.roots).toEqual([g1.root, g2.root]);
  });

  it('throws SchemaConflictError on divergent bodies for the same type-id', () => {
    const g1: SchemaGraph = { defs: new Map([['g.P', pointDef()]]), root: t.ref('g.P') };
    const g2: SchemaGraph = {
      defs: new Map([['g.P', { body: t.record([field('x', t.s32())]) }]]),
      root: t.ref('g.P'),
    };

    expect(() => mergeAgentGraphs([g1, g2])).toThrow(SchemaConflictError);
    try {
      mergeAgentGraphs([g1, g2]);
    } catch (e) {
      expect(e).toBeInstanceOf(SchemaConflictError);
      expect((e as SchemaConflictError).typeId).toBe('g.P');
    }
  });

  it('a merged agent graph round-trips through the WIT codec for every root', () => {
    const b1 = new SchemaBuilder();
    const r1 = b1.register('m.Tree', () =>
      t.variant([
        { name: 'leaf', payload: t.s32(), metadata: { aliases: [], examples: [] } },
        {
          name: 'node',
          payload: t.tuple([b1.ref('m.Tree'), b1.ref('m.Tree')]),
          metadata: { aliases: [], examples: [] },
        },
      ]),
    );
    const g1 = b1.buildGraph(r1);

    const b2 = new SchemaBuilder();
    const r2 = b2.register('m.Tree', () =>
      t.variant([
        { name: 'leaf', payload: t.s32(), metadata: { aliases: [], examples: [] } },
        {
          name: 'node',
          payload: t.tuple([b2.ref('m.Tree'), b2.ref('m.Tree')]),
          metadata: { aliases: [], examples: [] },
        },
      ]),
    );
    const g2 = b2.buildGraph(t.list(r2));

    const merged = mergeAgentGraphs([g1, g2]);
    expect([...merged.defs.keys()]).toEqual(['m.Tree']);

    for (const root of merged.roots) {
      const graph: SchemaGraph = { defs: merged.defs, root };
      expect(schemaGraphFromWit(schemaGraphToWit(graph))).toEqual(graph);
    }
  });
});
