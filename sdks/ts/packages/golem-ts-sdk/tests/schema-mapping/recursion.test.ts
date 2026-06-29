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

// Slice 3: recursive and mutually-recursive types are first-class.
//
// The `ResolvedType`-native mapper (`mapTsTypeToResolvedGraph`) walks the
// finite reflected `Type.Type` tree — in which the reflection emits a recursive
// back-edge as a `{ kind: 'others', recursive: true }` node — and produces an
// acyclic `ResolvedGraph` where recursion is expressed via `ref` ids. These
// tests drive the mapper with faithfully-reconstructed `Type.Type` inputs
// (via `buildTypeFromJSON`, the exact path the runtime uses to turn reflected
// metadata back into `Type.Type`) and prove, for a directly-recursive type, a
// mutually-recursive pair, and recursion through a tagged variant + tuple:
//   1. mapping yields exactly one `ref` def per recursive type (no infinite
//      expansion),
//   2. the projected `SchemaGraph` survives a WIT carrier round-trip,
//   3. finite values round-trip through `serialize -> schema-value-tree ->
//      deserialize` at several depths including the base case.

import { describe, it, expect } from 'vitest';
import fc from 'fast-check';
import { buildTypeFromJSON, LiteTypeJSON } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../src/newTypes/either';
import { mapTsTypeToResolvedGraph } from '../../src/internal/mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from '../../src/internal/mapping/types/schemaType';
import { ResolvedGraph } from '../../src/internal/mapping/types/resolvedType';
import { serializeGraph, deserializeGraph } from '../../src/internal/mapping/values/schemaValue';
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
} from '../../src/internal/schema-model';

// ============================================================
// Helpers
// ============================================================

function mapGraph(json: LiteTypeJSON): ResolvedGraph {
  const type = buildTypeFromJSON(json);
  const result = mapTsTypeToResolvedGraph(type, undefined);
  return Either.getOrThrowWith(result, (e) => new Error(`mapping failed: ${e}`));
}

/** Project a `ResolvedGraph` and assert it survives a WIT carrier round-trip. */
function projectGraph(graph: ResolvedGraph): ReturnType<typeof resolvedGraphToSchemaType> {
  const mapping = resolvedGraphToSchemaType(graph);
  const back = schemaGraphFromWit(schemaGraphToWit(mapping.graph));
  expect(back.defs.size).toBe(mapping.graph.defs.size);
  return mapping;
}

/** Round-trip a value through the graph-aware codec, directly and via the WIT carrier. */
function roundtrip<T>(value: T, graph: ResolvedGraph): void {
  const sv = serializeGraph(value, graph);
  expect(deserializeGraph(sv, graph)).toEqual(value);

  const sv2 = schemaValueFromWit(schemaValueToWit(sv));
  expect(deserializeGraph(sv2, graph)).toEqual(value);
}

function num(): LiteTypeJSON {
  return { kind: 'number', optional: false };
}

function str(): LiteTypeJSON {
  return { kind: 'string', optional: false };
}

/** A recursive back-edge to a named type, exactly as the reflection emits it. */
function recursiveRef(name: string, owner: string): LiteTypeJSON {
  return { kind: 'others', name, owner, optional: false, recursive: true };
}

// ============================================================
// Directly-recursive type: interface Tree { value: number; children: Tree[] }
// ============================================================

describe('recursion — directly-recursive type', () => {
  const treeJson: LiteTypeJSON = {
    kind: 'interface',
    name: 'Tree',
    owner: 'M',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'value', type: num() },
      {
        name: 'children',
        type: { kind: 'array', optional: false, element: recursiveRef('Tree', 'M') },
      },
    ],
  };

  it('maps to a single ref def, root is a ref (no infinite expansion)', () => {
    const graph = mapGraph(treeJson);

    expect(graph.root.body).toEqual({ tag: 'ref', id: 'M.Tree' });
    expect([...graph.defs.keys()]).toEqual(['M.Tree']);

    const def = graph.defs.get('M.Tree')!;
    expect(def.body.tag).toBe('record');
    if (def.body.tag === 'record') {
      const children = def.body.fields.find((f) => f.name === 'children')!;
      expect(children.type.body.tag).toBe('list');
      if (children.type.body.tag === 'list') {
        // The recursive back-edge is a ref, not an inlined copy of the record.
        expect(children.type.body.element.body).toEqual({ tag: 'ref', id: 'M.Tree' });
      }
    }
  });

  it('projects to a SchemaGraph with one ref def that survives a WIT round-trip', () => {
    const graph = mapGraph(treeJson);
    const mapping = projectGraph(graph);

    expect(mapping.root.body).toEqual({ tag: 'ref', id: 'M.Tree' });
    expect([...mapping.graph.defs.keys()]).toEqual(['M.Tree']);
  });

  it('round-trips finite values at several depths including the base case', () => {
    const graph = mapGraph(treeJson);

    // Base case: a leaf.
    roundtrip({ value: 1, children: [] }, graph);

    // Depth 2.
    roundtrip(
      {
        value: 1,
        children: [
          { value: 2, children: [] },
          { value: 3, children: [] },
        ],
      },
      graph,
    );

    // Depth 3, unbalanced.
    roundtrip(
      {
        value: 1,
        children: [
          { value: 2, children: [{ value: 4, children: [] }] },
          { value: 3, children: [] },
        ],
      },
      graph,
    );
  });

  it('round-trips randomly-generated finite trees (property-based)', () => {
    const graph = mapGraph(treeJson);

    // A bounded-depth recursive arbitrary: `tree` references itself through
    // `children`, with depth controlled so generated values stay finite.
    type Tree = { value: number; children: Tree[] };
    const { tree } = fc.letrec<{ tree: Tree }>((rec) => ({
      tree: fc.record({
        value: fc.integer(),
        children: fc.oneof(
          { depthSize: 'small', withCrossShrink: true },
          fc.constant([] as Tree[]),
          fc.array(rec('tree'), { maxLength: 3 }),
        ),
      }),
    }));

    fc.assert(
      fc.property(tree, (value) => {
        roundtrip(value, graph);
      }),
      { numRuns: 300 },
    );
  });
});

// ============================================================
// Mutually-recursive types: A <-> B
//   interface A { tag: string; next?: B }
//   interface B { count: number; items: A[] }
// ============================================================

describe('recursion — mutually-recursive types', () => {
  const bJson: LiteTypeJSON = {
    kind: 'interface',
    name: 'B',
    owner: 'M',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'count', type: num() },
      {
        name: 'items',
        type: { kind: 'array', optional: false, element: recursiveRef('A', 'M') },
      },
    ],
  };

  const aJson: LiteTypeJSON = {
    kind: 'interface',
    name: 'A',
    owner: 'M',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'tag', type: str() },
      { name: 'next', optional: true, type: bJson },
    ],
  };

  it('lands both defs in one graph, root is a ref to A', () => {
    const graph = mapGraph(aJson);

    expect(graph.root.body).toEqual({ tag: 'ref', id: 'M.A' });
    expect(new Set(graph.defs.keys())).toEqual(new Set(['M.A', 'M.B']));

    // A.next is an option wrapping a ref to B (optionality on the wrapper).
    const aDef = graph.defs.get('M.A')!;
    if (aDef.body.tag === 'record') {
      const next = aDef.body.fields.find((f) => f.name === 'next')!;
      expect(next.type.body.tag).toBe('option');
      if (next.type.body.tag === 'option') {
        expect(next.type.body.element.body).toEqual({ tag: 'ref', id: 'M.B' });
      }
    }

    // B.items is a list of refs back to A.
    const bDef = graph.defs.get('M.B')!;
    if (bDef.body.tag === 'record') {
      const items = bDef.body.fields.find((f) => f.name === 'items')!;
      if (items.type.body.tag === 'list') {
        expect(items.type.body.element.body).toEqual({ tag: 'ref', id: 'M.A' });
      }
    }
  });

  it('projects to a two-def SchemaGraph that survives a WIT round-trip', () => {
    const graph = mapGraph(aJson);
    const mapping = projectGraph(graph);
    expect(new Set(mapping.graph.defs.keys())).toEqual(new Set(['M.A', 'M.B']));
  });

  it('round-trips finite mutually-recursive values', () => {
    const graph = mapGraph(aJson);

    // Base case: A with no `next`.
    roundtrip({ tag: 'leaf', next: undefined }, graph);

    // A -> B -> [A, A].
    roundtrip(
      {
        tag: 'root',
        next: {
          count: 2,
          items: [
            { tag: 'child-1', next: undefined },
            { tag: 'child-2', next: { count: 0, items: [] } },
          ],
        },
      },
      graph,
    );
  });
});

// ============================================================
// Recursion through a tagged variant + tuple
//   type Expr = { tag: 'lit'; value: number }
//             | { tag: 'add'; operands: [Expr, Expr] }
// ============================================================

describe('recursion — through a tagged variant and tuple', () => {
  const litCase: LiteTypeJSON = {
    kind: 'object',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'tag', type: { kind: 'literal', literalValue: '"lit"', optional: false } },
      { name: 'value', type: num() },
    ],
  };

  const addCase: LiteTypeJSON = {
    kind: 'object',
    optional: false,
    typeParams: [],
    properties: [
      { name: 'tag', type: { kind: 'literal', literalValue: '"add"', optional: false } },
      {
        name: 'operands',
        type: {
          kind: 'tuple',
          optional: false,
          elements: [recursiveRef('Expr', 'M'), recursiveRef('Expr', 'M')],
        },
      },
    ],
  };

  const exprJson: LiteTypeJSON = {
    kind: 'union',
    name: 'Expr',
    owner: 'M',
    optional: false,
    originalTypeName: undefined,
    typeParams: [],
    types: [litCase, addCase],
  };

  it('maps to a single tagged-variant ref def with refs closing the cycle', () => {
    const graph = mapGraph(exprJson);

    expect(graph.root.body).toEqual({ tag: 'ref', id: 'M.Expr' });
    expect([...graph.defs.keys()]).toEqual(['M.Expr']);

    const def = graph.defs.get('M.Expr')!;
    expect(def.body.tag).toBe('variant');
    if (def.body.tag === 'variant') {
      expect(def.body.tagged).toBe(true);
      const add = def.body.cases.find((c) => c.name === 'add')!;
      expect(add.payload?.body.tag).toBe('tuple');
      if (add.payload?.body.tag === 'tuple') {
        expect(add.payload.body.elements.map((e) => e.body)).toEqual([
          { tag: 'ref', id: 'M.Expr' },
          { tag: 'ref', id: 'M.Expr' },
        ]);
      }
    }
  });

  it('projects to a SchemaGraph that survives a WIT round-trip', () => {
    const graph = mapGraph(exprJson);
    const mapping = projectGraph(graph);
    expect([...mapping.graph.defs.keys()]).toEqual(['M.Expr']);
  });

  it('round-trips nested expression values', () => {
    const graph = mapGraph(exprJson);

    roundtrip({ tag: 'lit', value: 42 }, graph);

    roundtrip(
      {
        tag: 'add',
        operands: [
          { tag: 'lit', value: 1 },
          {
            tag: 'add',
            operands: [
              { tag: 'lit', value: 2 },
              { tag: 'lit', value: 3 },
            ],
          },
        ],
      },
      graph,
    );
  });
});

// ============================================================
// Unresolvable recursion is rejected with a clear message
// ============================================================

describe('recursion — unresolvable back-edge', () => {
  it('rejects a recursive reference whose definition is not a registered composite', () => {
    // A bare recursive `others` node with no enclosing composite on the stack:
    // there is nothing to reference, so it must be rejected (not silently
    // produce a dangling ref).
    const orphan: LiteTypeJSON = {
      kind: 'others',
      name: 'Foo',
      owner: 'M',
      optional: false,
      recursive: true,
    };

    const result = mapTsTypeToResolvedGraph(buildTypeFromJSON(orphan), undefined);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) {
      expect(result.val).toContain('recursive');
    }
  });
});
