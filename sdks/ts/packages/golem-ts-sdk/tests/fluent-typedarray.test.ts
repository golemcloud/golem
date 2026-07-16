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
import { compileSchema } from '../src/fluent/schema/adapter';
import { s } from '../src/fluent/schema/markers';

// Each fluent typed-array marker → WIT `list<primN>`, decoded to the concrete subclass.
const KINDS = [
  {
    name: 'int8Array',
    make: () => s.int8Array(),
    ctor: Int8Array,
    prim: 's8',
    sample: [-1, 2, 127],
  },
  {
    name: 'uint8Array',
    make: () => s.uint8Array(),
    ctor: Uint8Array,
    prim: 'u8',
    sample: [0, 255, 7],
  },
  { name: 'bytes', make: () => s.bytes(), ctor: Uint8Array, prim: 'u8', sample: [1, 2, 3] },
  {
    name: 'int16Array',
    make: () => s.int16Array(),
    ctor: Int16Array,
    prim: 's16',
    sample: [-100, 200],
  },
  {
    name: 'uint16Array',
    make: () => s.uint16Array(),
    ctor: Uint16Array,
    prim: 'u16',
    sample: [0, 65535],
  },
  {
    name: 'int32Array',
    make: () => s.int32Array(),
    ctor: Int32Array,
    prim: 's32',
    sample: [-5, 5, 1000000],
  },
  {
    name: 'uint32Array',
    make: () => s.uint32Array(),
    ctor: Uint32Array,
    prim: 'u32',
    sample: [0, 4000000000],
  },
  {
    name: 'float32Array',
    make: () => s.float32Array(),
    ctor: Float32Array,
    prim: 'f32',
    sample: [1.5, -2.5, 0.25],
  },
  {
    name: 'float64Array',
    make: () => s.float64Array(),
    ctor: Float64Array,
    prim: 'f64',
    sample: [3.14, -1],
  },
  {
    name: 'bigInt64Array',
    make: () => s.bigInt64Array(),
    ctor: BigInt64Array,
    prim: 's64',
    sample: [-1n, 9007199254740993n],
  },
  {
    name: 'bigUint64Array',
    make: () => s.bigUint64Array(),
    ctor: BigUint64Array,
    prim: 'u64',
    sample: [0n, 18446744073709551615n],
  },
] as const;

describe('fluent typed-array markers → WIT list<primN>', () => {
  for (const k of KINDS) {
    it(`${k.name} → list<${k.prim}>, round-trips a ${k.ctor.name}`, () => {
      const codec = compileSchema(k.make());

      const rootBody = codec.graph.root.body as { tag: string; element: { body: { tag: string } } };
      expect(rootBody.tag).toBe('list');
      expect(rootBody.element.body.tag).toBe(k.prim);

      const input = new (k.ctor as new (src: readonly (number | bigint)[]) => object)(k.sample);
      const wire = codec.toValue(input) as { tag: string; elements: unknown[] };
      expect(wire.tag).toBe('list');
      expect(wire.elements).toHaveLength(k.sample.length);

      const decoded = codec.fromValue(wire);
      expect(decoded).toBeInstanceOf(k.ctor);
      expect(Array.from(decoded as Iterable<number | bigint>)).toEqual(Array.from(k.sample));
    });
  }
});
