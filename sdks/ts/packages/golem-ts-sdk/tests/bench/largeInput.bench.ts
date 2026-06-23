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

// Isolated micro-benchmark for the "large input" invocation path. The
// `throughput-large-input` platform benchmark sends a large list as the
// invocation argument (the benchmark agent receives a `Uint8Array`, i.e. a
// `list<u8>`); the value size — not its element type — is what matters.
//
// The full per-invocation value path on the agent side is:
//
//   TS value  --serializeGraph-->  SchemaValue  --schemaValueToWit-->  wire
//   wire  --schemaValueFromWit-->  SchemaValue  --deserializeGraph-->  TS value
//
// The existing `conversion.bench.ts` only covers the inner
// `schemaValueToWit` / `schemaValueFromWit` legs and tops out at 100 elements,
// so it does not exercise the `serializeGraph` / `deserializeGraph` mapping
// layer at scale. This file benchmarks every leg separately, plus the encode
// and decode round-trips, for large lists so we can see which stage dominates.

import { bench, describe } from './harness';
import { SchemaValue, schemaValueToWit, schemaValueFromWit } from '../../src/internal/schema-model';
import {
  serializeGraph,
  deserializeGraph,
  serializeGraphToWit,
  deserializeGraphFromWit,
} from '../../src/internal/mapping/values/schemaValue';
import { r, ResolvedGraph } from '../../src/internal/mapping/types/resolvedType';

const TIME = 1000;

const SIZES = [10_000, 50_000];

// `list<u8>` carried as a `Uint8Array` — mirrors the benchmark agent's
// `largeInput(input: Uint8Array)` endpoint exactly.
const u8Graph: ResolvedGraph = { defs: new Map(), root: r.list(r.u8(), 'u8') };
// `list<u32>` carried as a plain `number[]` — the generic (non-typed-array)
// large-list path that most user-defined large inputs take.
const u32Graph: ResolvedGraph = { defs: new Map(), root: r.list(r.u32()) };

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

const fixtures: Fixture[] = [];
for (const n of SIZES) {
  fixtures.push(makeFixture(`u8[${n}] (Uint8Array)`, u8Graph, new Uint8Array(n)));
  fixtures.push(
    makeFixture(
      `u32[${n}] (number[])`,
      u32Graph,
      Array.from({ length: n }, (_, i) => i & 0xffff),
    ),
  );
}

// Stage 1: TS value -> SchemaValue
describe('large input: serializeGraph (TS value -> SchemaValue)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void serializeGraph(f.tsValue, f.graph), { time: TIME });
  }
});

// Stage 2: SchemaValue -> wire
describe('large input: schemaValueToWit (SchemaValue -> wire)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void schemaValueToWit(f.schemaValue), { time: TIME });
  }
});

// Stage 3: wire -> SchemaValue
describe('large input: schemaValueFromWit (wire -> SchemaValue)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void schemaValueFromWit(f.wit), { time: TIME });
  }
});

// Stage 4: SchemaValue -> TS value
describe('large input: deserializeGraph (SchemaValue -> TS value)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void deserializeGraph(f.schemaValue, f.graph), { time: TIME });
  }
});

// Full encode (producing/returning a large value): TS value -> wire
describe('large input: encode round-trip (TS value -> wire)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void schemaValueToWit(serializeGraph(f.tsValue, f.graph)), { time: TIME });
  }
});

// Full decode (receiving a large argument): wire -> TS value. This is the leg
// the benchmark agent actually runs for `largeInput`.
describe('large input: decode round-trip (wire -> TS value)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void deserializeGraph(schemaValueFromWit(f.wit), f.graph), { time: TIME });
  }
});

// Fused single-pass encode: TS value -> wire, skipping the intermediate
// SchemaValue tree. Compare against `encode round-trip` above (same fixtures).
describe('large input: encode round-trip FUSED (TS value -> wire)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void serializeGraphToWit(f.tsValue, f.graph), { time: TIME });
  }
});

// Fused single-pass decode: wire -> TS value. Compare against `decode
// round-trip` above (same fixtures).
describe('large input: decode round-trip FUSED (wire -> TS value)', () => {
  for (const f of fixtures) {
    bench(f.label, () => void deserializeGraphFromWit(f.wit, f.graph), { time: TIME });
  }
});
