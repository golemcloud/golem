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
import fc from 'fast-check';

import {
  schemaGraphToWit,
  schemaGraphFromWit,
  schemaValueToWit,
  schemaValueFromWit,
  typedSchemaValueToWit,
  typedSchemaValueFromWit,
} from '../../src/internal/schema-model';

import { arbSchemaGraph, arbSchemaValue, arbTypedSchemaValue } from './arbitraries';

const RUNS = 500;

describe('schema-model WIT codec roundtrip (property-based)', () => {
  it('schema graph: fromWit(toWit(g)) deep-equals g', () => {
    fc.assert(
      fc.property(arbSchemaGraph, (graph) => {
        const back = schemaGraphFromWit(schemaGraphToWit(graph));
        expect(back).toEqual(graph);
      }),
      { numRuns: RUNS },
    );
  });

  it('schema value: fromWit(toWit(v)) deep-equals v', () => {
    fc.assert(
      fc.property(arbSchemaValue, (value) => {
        const back = schemaValueFromWit(schemaValueToWit(value));
        expect(back).toEqual(value);
      }),
      { numRuns: RUNS },
    );
  });

  it('typed schema value: fromWit(toWit(tv)) deep-equals tv', () => {
    fc.assert(
      fc.property(arbTypedSchemaValue, (tv) => {
        const back = typedSchemaValueFromWit(typedSchemaValueToWit(tv));
        expect(back).toEqual(tv);
      }),
      { numRuns: RUNS },
    );
  });

  it('schema graph: toWit is idempotent through a round-trip (toWit∘fromWit∘toWit == toWit)', () => {
    fc.assert(
      fc.property(arbSchemaGraph, (graph) => {
        const wit1 = schemaGraphToWit(graph);
        const wit2 = schemaGraphToWit(schemaGraphFromWit(wit1));
        expect(wit2).toEqual(wit1);
      }),
      { numRuns: RUNS },
    );
  });

  it('schema value: toWit is idempotent through a round-trip', () => {
    fc.assert(
      fc.property(arbSchemaValue, (value) => {
        const wit1 = schemaValueToWit(value);
        const wit2 = schemaValueToWit(schemaValueFromWit(wit1));
        expect(wit2).toEqual(wit1);
      }),
      { numRuns: RUNS },
    );
  });
});

describe('schema-model WIT carrier structural invariants', () => {
  it('flattened graph has in-range indices and sorted defs', () => {
    fc.assert(
      fc.property(arbSchemaGraph, (graph) => {
        const wit = schemaGraphToWit(graph);
        const n = wit.typeNodes.length;
        expect(wit.root).toBeGreaterThanOrEqual(0);
        expect(wit.root).toBeLessThan(n);

        // defs are sorted by id and their bodies point in range
        const ids = wit.defs.map((d) => d.id);
        expect(ids).toEqual([...ids].sort());
        for (const d of wit.defs) {
          expect(d.body).toBeGreaterThanOrEqual(0);
          expect(d.body).toBeLessThan(n);
        }

        // every ref-type points at a valid def index
        for (const node of wit.typeNodes) {
          if (node.body.tag === 'ref-type') {
            expect(node.body.val).toBeGreaterThanOrEqual(0);
            expect(node.body.val).toBeLessThan(wit.defs.length);
          }
        }
      }),
      { numRuns: RUNS },
    );
  });

  it('flattened value tree has in-range root and child indices', () => {
    fc.assert(
      fc.property(arbSchemaValue, (value) => {
        const wit = schemaValueToWit(value);
        const n = wit.valueNodes.length;
        expect(wit.root).toBeGreaterThanOrEqual(0);
        expect(wit.root).toBeLessThan(n);
        for (const node of wit.valueNodes) {
          switch (node.tag) {
            case 'record-value':
            case 'tuple-value':
            case 'list-value':
            case 'fixed-list-value':
              for (const i of node.val) {
                expect(i).toBeGreaterThanOrEqual(0);
                expect(i).toBeLessThan(n);
              }
              break;
            case 'map-value':
              for (const e of node.val) {
                expect(e.key).toBeLessThan(n);
                expect(e.value).toBeLessThan(n);
              }
              break;
            default:
              break;
          }
        }
      }),
      { numRuns: RUNS },
    );
  });
});
