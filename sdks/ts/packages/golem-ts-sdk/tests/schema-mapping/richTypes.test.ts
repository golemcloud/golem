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

import { describe, expect, it } from 'vitest';
import { Duration, Path, Quantity } from '../../src/richTypes';
import { r, ResolvedGraph } from '../../src/internal/mapping/types/resolvedType';
import { deserializeGraph, serializeGraph } from '../../src/internal/mapping/values/schemaValue';

function graph(root: ResolvedGraph['root']): ResolvedGraph {
  return { defs: new Map(), root };
}

describe('rich semantic value codecs', () => {
  it('round-trips path values', () => {
    const g = graph(r.path({ direction: 'in-out', kind: 'any' }));
    const encoded = serializeGraph(new Path('/tmp/input.txt'), g);
    expect(encoded).toEqual({ tag: 'path', value: '/tmp/input.txt' });
    expect(deserializeGraph(encoded, g)).toEqual(new Path('/tmp/input.txt'));
  });

  it('round-trips url values', () => {
    const g = graph(r.url({}));
    const encoded = serializeGraph(new URL('https://example.com/a'), g);
    expect(encoded).toEqual({ tag: 'url', value: 'https://example.com/a' });
    expect(deserializeGraph(encoded, g).toString()).toBe('https://example.com/a');
  });

  it('round-trips datetime values', () => {
    const g = graph(r.datetime());
    const date = new Date('2026-06-24T12:34:56.789Z');
    const encoded = serializeGraph(date, g);
    expect(encoded).toEqual({
      tag: 'datetime',
      value: { seconds: 1782304496n, nanoseconds: 789000000 },
    });
    expect(deserializeGraph(encoded, g)).toEqual(date);
  });

  it('round-trips pre-epoch datetime values', () => {
    const g = graph(r.datetime());
    const date = new Date(-1);
    const encoded = serializeGraph(date, g);
    expect(encoded).toEqual({
      tag: 'datetime',
      value: { seconds: -1n, nanoseconds: 999000000 },
    });
    expect(deserializeGraph(encoded, g)).toEqual(date);
  });

  it('round-trips duration values', () => {
    const g = graph(r.duration());
    const encoded = serializeGraph(new Duration(-42n), g);
    expect(encoded).toEqual({ tag: 'duration', nanoseconds: -42n });
    expect(deserializeGraph(encoded, g)).toEqual(new Duration(-42n));
  });

  it('round-trips quantity values', () => {
    const g = graph(r.quantity({ baseUnit: 'm', allowedSuffixes: ['m', 'cm'] }));
    const value = { mantissa: 123n, scale: 2, unit: 'm' };
    const encoded = serializeGraph(new Quantity(value), g);
    expect(encoded).toEqual({ tag: 'quantity', value });
    expect(deserializeGraph(encoded, g)).toEqual(new Quantity(value));
  });
});
