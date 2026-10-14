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
import { compileSchema } from '../src/schema/adapter';
import { schemaValueToWit, type SchemaValue } from '../src/internal/schema-model';
import {
  SchemaValueWriter,
  invocationSchemaValueReader,
  directSchemaValueToWit,
  directSchemaValueFromWit,
} from '../src/schema/codec';
import { s } from '../src/schema/markers';

// Each typed-array marker → WIT `list<primN>`, decoded to the concrete subclass.
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

describe('typed-array markers → WIT list<primN>', () => {
  for (const k of KINDS) {
    it(`${k.name} → list<${k.prim}>, round-trips a ${k.ctor.name}`, () => {
      const codec = compileSchema(k.make());

      const rootBody = codec.graph.root.body as { tag: string; element: { body: { tag: string } } };
      expect(rootBody.tag).toBe('list');
      expect(rootBody.element.body.tag).toBe(k.prim);

      const input = new (k.ctor as new (src: readonly (number | bigint)[]) => object)(k.sample);
      const wire = codec.toValue(input) as SchemaValue & { elements: SchemaValue[] };
      expect(wire.tag).toBe('list');
      expect(wire.elements).toHaveLength(k.sample.length);

      const decoded = codec.fromValue(wire);
      expect(decoded).toBeInstanceOf(k.ctor);
      expect(Array.from(decoded as Iterable<number | bigint>)).toEqual(
        Array.from(k.sample as readonly (number | bigint)[]),
      );

      expect(directSchemaValueFromWit(codec, directSchemaValueToWit(codec, input))).toEqual(input);
      const writer = new SchemaValueWriter();
      const root = codec.invocationDirect!.write(input, writer)!;
      expect(
        codec.invocationDirect!.read(
          invocationSchemaValueReader({ valueNodes: writer.valueNodes, root }),
          root,
        ),
      ).toEqual(input);
    });
  }

  it.each([0, 1, 255, 256, 10000])(
    'preserves bytes and full-width indices at length %i',
    (length) => {
      const codec = compileSchema(s.uint8Array());
      const input = Uint8Array.from({ length }, (_, i) => (i * 37 + 19) % 256);
      const writer = new SchemaValueWriter();
      // A preceding argument must not truncate the following list's arena indices.
      for (let i = 0; i < 300; i++) writer.add({ tag: 'u8-value', val: i % 256 });
      const root = codec.invocationDirect!.write(input, writer)!;
      expect(writer.valueNodes.slice(300)).toEqual(
        schemaValueToWit(codec.toValue(input)).valueNodes.map((node) =>
          node.tag === 'list-value' ? { ...node, val: node.val.map((i) => i + 300) } : node,
        ),
      );
      const decoded = codec.invocationDirect!.read(
        invocationSchemaValueReader({ valueNodes: writer.valueNodes, root }),
        root,
      );
      expect(decoded).toBeInstanceOf(Uint8Array);
      expect(decoded).toEqual(input);
    },
  );

  it('rejects the wrong array kind and malformed scalar carriers', () => {
    const codec = compileSchema(s.uint8Array());
    for (const input of [[1, 2], new Int8Array([1, 2]), 'abc']) {
      expect(() => codec.invocationDirect!.write(input, new SchemaValueWriter())).toThrow();
    }
    expect(() =>
      invocationSchemaValueReader({
        valueNodes: [
          { tag: 'u8-value', val: 256 },
          { tag: 'list-value', val: [0] },
        ],
        root: 1,
      }),
    ).toThrow();
  });
});
