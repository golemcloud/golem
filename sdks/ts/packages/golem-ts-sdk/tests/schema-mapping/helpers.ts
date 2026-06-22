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

import { expect } from 'vitest';
import { resolvedGraphToSchemaType } from '../../src/internal/mapping/types/schemaType';
import { ResolvedGraph } from '../../src/internal/mapping/types/resolvedType';
import { deserializeGraph, serializeGraph } from '../../src/internal/mapping/values/schemaValue';
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  SchemaValue,
} from '../../src/internal/schema-model';
import { TypePair } from '../testUtils';

/**
 * Round-trips a TypeScript value through the production schema-value codec guided
 * by the {@link ResolvedGraph} produced by the production mapper, and asserts
 * equality both directly and after a full WIT carrier round-trip.
 */
export function roundtripValue<T>(data: T, graph: ResolvedGraph): SchemaValue {
  const sv = serializeGraph(data, graph);
  expect(deserializeGraph(sv, graph)).toEqual(data);

  // Through the flat WIT schema-value-tree carrier and back.
  const sv2 = schemaValueFromWit(schemaValueToWit(sv));
  expect(deserializeGraph(sv2, graph)).toEqual(data);

  return sv;
}

/** Round-trips the value of a `getXxxType()` test pair. */
export function roundtripPair<T>(data: T, pair: TypePair): SchemaValue {
  return roundtripValue(data, pair[0]);
}

/**
 * Project a resolved graph into a schema graph and assert the graph itself
 * survives a WIT carrier round-trip (a structural-validity check on the
 * projection). Returns the projection for further assertions.
 */
export function project(graph: ResolvedGraph): ReturnType<typeof resolvedGraphToSchemaType> {
  const mapping = resolvedGraphToSchemaType(graph);
  const back = schemaGraphFromWit(schemaGraphToWit(mapping.graph));
  expect(back.defs.size).toBe(mapping.graph.defs.size);
  return mapping;
}

/** Project a `getXxxType()` test pair's graph. */
export function projectPair(pair: TypePair): ReturnType<typeof resolvedGraphToSchemaType> {
  return project(pair[0]);
}
