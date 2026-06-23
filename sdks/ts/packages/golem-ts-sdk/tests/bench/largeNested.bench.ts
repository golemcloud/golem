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

// Large nested / recursive companions to `largeInput.bench.ts`. Where
// `largeInput` sends a single big flat list, these exercise the same full
// per-invocation value path
//
//   TS value  --serializeGraph-->  SchemaValue  --schemaValueToWit-->  wire
//   wire  --schemaValueFromWit-->  SchemaValue  --deserializeGraph-->  TS value
//
// but for deeply structured inputs at scale:
//
//   * a large recursive tree (`record { value: u32; children: list<ref> }`),
//     whose `ref` back-edges force `resolveRef` down its cycle-guarding loop on
//     every node and every child — the branch that the non-`ref` fast path does
//     NOT cover, so this guards against regressing ref-heavy workloads; and
//   * a large non-recursive nested record (a record holding a big list of small
//     records), which stays on the fast path but stresses nested record/list
//     traversal.
//
// `conversion.bench.ts` already covers nested/recursive shapes, but only through
// the inner `schemaValueToWit` / `schemaValueFromWit` wire codec at a fixed,
// small size; these add the `serializeGraph` / `deserializeGraph` mapping layer
// at scale.

import { bench, describe } from './harness';
import { SchemaValue, schemaValueToWit, schemaValueFromWit } from '../../src/internal/schema-model';
import {
  serializeGraph,
  deserializeGraph,
  serializeGraphToWit,
  deserializeGraphFromWit,
} from '../../src/internal/mapping/values/schemaValue';
import {
  r,
  resolvedField,
  ResolvedGraph,
  TypeId,
} from '../../src/internal/mapping/types/resolvedType';

const TIME = 1000;

const SIZES = [10_000, 50_000];

// ---------------------------------------------------------------------------
// Recursive tree graph: `interface Tree { value: u32; children: Tree[] }`,
// encoded exactly like the reflection mapper does — a single `ref` def with the
// root and the `children` element both pointing back at it.
// ---------------------------------------------------------------------------

const TREE_ID: TypeId = 'M.Tree';
const treeNode = r.record(
  [resolvedField('value', r.u32()), resolvedField('children', r.list(r.ref(TREE_ID)))],
  'Tree',
  'M',
);
const treeGraph: ResolvedGraph = {
  defs: new Map([[TREE_ID, treeNode]]),
  root: r.ref(TREE_ID),
};

interface TreeValue {
  value: number;
  children: TreeValue[];
}

// Build a roughly balanced tree with exactly `nodes` nodes (breadth-first fill).
function buildTree(nodes: number, breadth = 4): TreeValue {
  let counter = 0;
  const make = (): TreeValue => ({ value: counter++ & 0xffff, children: [] });
  const root = make();
  let created = 1;
  const frontier: TreeValue[] = [root];
  while (created < nodes && frontier.length > 0) {
    const node = frontier.shift()!;
    for (let i = 0; i < breadth && created < nodes; i++) {
      const child = make();
      node.children.push(child);
      frontier.push(child);
      created++;
    }
  }
  return root;
}

// ---------------------------------------------------------------------------
// Non-recursive nested record graph:
//   record { name: string; items: list<record { id: u32; label: string }> }
// ---------------------------------------------------------------------------

const itemNode = r.record(
  [resolvedField('id', r.u32()), resolvedField('label', r.string())],
  'Item',
  'M',
);
const nestedGraph: ResolvedGraph = {
  defs: new Map(),
  root: r.record(
    [resolvedField('name', r.string()), resolvedField('items', r.list(itemNode))],
    'Container',
    'M',
  ),
};

function buildNested(items: number): unknown {
  return {
    name: 'container',
    items: Array.from({ length: items }, (_, i) => ({ id: i & 0xffff, label: `item-${i}` })),
  };
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

interface Fixture {
  label: string;
  graph: ResolvedGraph;
  tsValue: unknown;
  schemaValue: SchemaValue;
  wit: ReturnType<typeof schemaValueToWit>;
}

function makeFixture(label: string, graph: ResolvedGraph, tsValue: unknown): Fixture {
  const schemaValue = serializeGraph(tsValue, graph);
  const wit = schemaValueToWit(schemaValue);
  return { label, graph, tsValue, schemaValue, wit };
}

const treeFixtures: Fixture[] = SIZES.map((n) =>
  makeFixture(`tree[${n} nodes]`, treeGraph, buildTree(n)),
);
const nestedFixtures: Fixture[] = SIZES.map((n) =>
  makeFixture(`nested[${n} items]`, nestedGraph, buildNested(n)),
);

function legs(title: string, fixtures: Fixture[]): void {
  describe(`${title}: serializeGraph (TS value -> SchemaValue)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void serializeGraph(f.tsValue, f.graph), { time: TIME });
    }
  });

  describe(`${title}: deserializeGraph (SchemaValue -> TS value)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void deserializeGraph(f.schemaValue, f.graph), { time: TIME });
    }
  });

  describe(`${title}: schemaValueToWit (SchemaValue -> wire)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void schemaValueToWit(f.schemaValue), { time: TIME });
    }
  });

  describe(`${title}: schemaValueFromWit (wire -> SchemaValue)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void schemaValueFromWit(f.wit), { time: TIME });
    }
  });

  describe(`${title}: encode round-trip (TS value -> wire)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void schemaValueToWit(serializeGraph(f.tsValue, f.graph)), {
        time: TIME,
      });
    }
  });

  describe(`${title}: decode round-trip (wire -> TS value)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void deserializeGraph(schemaValueFromWit(f.wit), f.graph), {
        time: TIME,
      });
    }
  });

  describe(`${title}: encode round-trip FUSED (TS value -> wire)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void serializeGraphToWit(f.tsValue, f.graph), { time: TIME });
    }
  });

  describe(`${title}: decode round-trip FUSED (wire -> TS value)`, () => {
    for (const f of fixtures) {
      bench(f.label, () => void deserializeGraphFromWit(f.wit, f.graph), { time: TIME });
    }
  });
}

legs('large recursive tree', treeFixtures);
legs('large nested record', nestedFixtures);
