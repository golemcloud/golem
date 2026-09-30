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

import { Uuid } from '../src/uuid';
import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  t,
  v,
} from '../src/internal/schema-model';

describe('UUID schema support', () => {
  const uuid = new Uuid(0x0011223344556677n, 0x8899aabbccddeeffn);

  it('uses the first-class WIT schema type and value cases', () => {
    const graph = { defs: new Map(), root: t.uuid() };
    expect(schemaGraphToWit(graph).typeNodes[0].body).toEqual({ tag: 'uuid-type' });
    expect(schemaGraphFromWit(schemaGraphToWit(graph)).root.body).toEqual({ tag: 'uuid' });

    const tree = schemaValueToWit(v.uuid(uuid));
    expect(tree.valueNodes[0]).toEqual({ tag: 'uuid-value', val: uuid });
    const decoded = schemaValueFromWit(tree);
    expect(decoded).toEqual(v.uuid(uuid));
    expect(decoded.tag === 'uuid' && decoded.value).toBeInstanceOf(Uuid);
  });

  it('round-trips the native Uuid through s.uuid()', () => {
    const codec = compileSchema(s.uuid());
    expect(codec.graph.root.body).toEqual({ tag: 'uuid' });
    expect(codec.toValue(uuid)).toEqual({ tag: 'uuid', value: uuid });
    expect(codec.fromValue(codec.toValue(uuid))).toBe(uuid);
  });

  it('rejects UUID halves outside the WIT u64 range', () => {
    expect(s.uuid()['~standard'].validate(new Uuid(-1n, 0n))).toEqual(
      expect.objectContaining({ issues: expect.any(Array) }),
    );
    expect(s.uuid()['~standard'].validate(new Uuid(0n, 1n << 64n))).toEqual(
      expect.objectContaining({ issues: expect.any(Array) }),
    );
  });
});
