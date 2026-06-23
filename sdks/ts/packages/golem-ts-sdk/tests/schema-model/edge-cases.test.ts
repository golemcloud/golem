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

import type {
  SchemaGraph as WitSchemaGraph,
  SchemaValueTree as WitSchemaValueTree,
} from 'golem:core/types@2.0.0';

import {
  GuestQuotaTokenHandle,
  SchemaBuilder,
  type SchemaGraph,
  type SchemaValue,
  emptyMetadata,
  field,
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  t,
  v,
  variantCase,
} from '../../src/internal/schema-model';

function roundtripValue(value: SchemaValue): void {
  expect(schemaValueFromWit(schemaValueToWit(value))).toEqual(value);
}

function roundtripGraph(graph: SchemaGraph): void {
  expect(schemaGraphFromWit(schemaGraphToWit(graph))).toEqual(graph);
}

describe('recursive and mutually-recursive types', () => {
  it('self-recursive Tree closes via reserve/commit and emits one def', () => {
    const b = new SchemaBuilder();
    const tree = b.register('user.Tree', () =>
      t.variant([
        variantCase('Leaf', t.s32()),
        variantCase('Node', t.tuple([b.ref('user.Tree'), b.ref('user.Tree')])),
      ]),
    );
    const graph = b.buildGraph(tree);

    const wit = schemaGraphToWit(graph);
    expect(wit.defs.length).toBe(1);
    expect(wit.defs[0].id).toBe('user.Tree');
    roundtripGraph(graph);
  });

  it('mutually-recursive A <-> B both land in one graph', () => {
    const b = new SchemaBuilder();
    const a = b.register('m.A', () =>
      t.record([
        field(
          'b',
          b.register('m.B', () => t.record([field('a', b.ref('m.A'))])),
        ),
      ]),
    );
    const graph = b.buildGraph(a);

    const wit = schemaGraphToWit(graph);
    expect(wit.defs.map((d) => d.id).sort()).toEqual(['m.A', 'm.B']);
    roundtripGraph(graph);
  });

  it('recursive value (a Node tree) round-trips', () => {
    // Node(Leaf(1), Node(Leaf(2), Leaf(3)))
    const leaf = (n: number): SchemaValue => v.variant(0, v.s32(n));
    const node = (l: SchemaValue, r: SchemaValue): SchemaValue => v.variant(1, v.tuple([l, r]));
    roundtripValue(node(leaf(1), node(leaf(2), leaf(3))));
  });
});

describe('definition deduplication and sharing', () => {
  it('a type referenced from multiple positions is deduplicated to one def', () => {
    const b = new SchemaBuilder();
    const point = b.register('g.Point', () => t.record([field('x', t.f64()), field('y', t.f64())]));
    const line = t.record([field('start', point), field('end', point)]);
    const graph = b.buildGraph(line);

    const wit = schemaGraphToWit(graph);
    expect(wit.defs.length).toBe(1);
    const refNodes = wit.typeNodes.filter((node) => node.body.tag === 'ref-type');
    expect(refNodes.length).toBe(2);
    roundtripGraph(graph);
  });

  it('register is idempotent for the same id', () => {
    const b = new SchemaBuilder();
    const r1 = b.register('x.Foo', () => t.record([field('a', t.bool())]));
    const r2 = b.register('x.Foo', () => t.record([field('b', t.string())]));
    // second registration is ignored; both return a ref to the same id
    expect(r1).toEqual(r2);
    const graph = b.buildGraph(r1);
    const wit = schemaGraphToWit(graph);
    expect(wit.defs.length).toBe(1);
    // the committed body is the first one
    const def = graph.defs.get('x.Foo')!;
    expect(def.body.body).toEqual({ tag: 'record', fields: [field('a', t.bool())] });
  });
});

describe('anonymous vs named placement', () => {
  it('anonymous composites get no def slot', () => {
    const graph: SchemaGraph = {
      defs: new Map(),
      root: t.list(t.option(t.tuple([t.s32(), t.string()]))),
    };
    const wit = schemaGraphToWit(graph);
    expect(wit.defs.length).toBe(0);
    roundtripGraph(graph);
  });
});

describe('empty and degenerate composites', () => {
  it('empty record / variant / enum / flags types round-trip', () => {
    roundtripGraph({ defs: new Map(), root: t.record([]) });
    roundtripGraph({ defs: new Map(), root: t.variant([]) });
    roundtripGraph({ defs: new Map(), root: t.enum([]) });
    roundtripGraph({ defs: new Map(), root: t.flags([]) });
  });

  it('zero-length fixed-list round-trips (type and value)', () => {
    roundtripGraph({ defs: new Map(), root: t.fixedList(t.bool(), 0) });
    roundtripValue(v.fixedList([]));
  });

  it('result with unit ok/err round-trips (type and value)', () => {
    roundtripGraph({ defs: new Map(), root: t.result(undefined, undefined) });
    roundtripValue(v.ok());
    roundtripValue(v.err());
  });

  it('empty record / list / tuple / flags / map values round-trip', () => {
    roundtripValue(v.record([]));
    roundtripValue(v.list([]));
    roundtripValue(v.tuple([]));
    roundtripValue(v.flags([]));
    roundtripValue(v.map([]));
  });
});

describe('option nesting distinguishes some(none) from none', () => {
  it('none vs some(none) vs some(some(x)) round-trip distinctly', () => {
    const none = v.option();
    const someNone = v.option(v.option());
    const someSome = v.option(v.option(v.s32(7)));

    roundtripValue(none);
    roundtripValue(someNone);
    roundtripValue(someSome);

    // they must encode differently
    const noneWit = schemaValueToWit(none);
    const someNoneWit = schemaValueToWit(someNone);
    expect(noneWit).not.toEqual(someNoneWit);
    expect(schemaValueFromWit(someNoneWit)).not.toEqual(none);
  });

  it('deeply nested lists round-trip', () => {
    roundtripGraph({ defs: new Map(), root: t.list(t.list(t.list(t.s32()))) });
    roundtripValue(v.list([v.list([v.list([v.s32(1), v.s32(2)])])]));
  });
});

describe('map keys', () => {
  it('string-keyed map round-trips', () => {
    roundtripValue(
      v.map([
        { key: v.string('a'), value: v.s32(1) },
        { key: v.string('b'), value: v.s32(2) },
      ]),
    );
  });

  it('non-string-keyed map round-trips', () => {
    roundtripValue(
      v.map([
        { key: v.s32(1), value: v.string('a') },
        { key: v.tuple([v.bool(true), v.u8(9)]), value: v.string('b') },
      ]),
    );
  });
});

describe('numeric boundaries', () => {
  it('signed/unsigned integer extremes round-trip', () => {
    roundtripValue(v.s8(-128));
    roundtripValue(v.s8(127));
    roundtripValue(v.s16(-32768));
    roundtripValue(v.s16(32767));
    roundtripValue(v.s32(-2147483648));
    roundtripValue(v.s32(2147483647));
    roundtripValue(v.u8(255));
    roundtripValue(v.u16(65535));
    roundtripValue(v.u32(4294967295));
    roundtripValue(v.s64(-(2n ** 63n)));
    roundtripValue(v.s64(2n ** 63n - 1n));
    roundtripValue(v.u64(0n));
    roundtripValue(v.u64(2n ** 64n - 1n));
  });

  it('float specials (NaN, +/-Inf, -0) round-trip', () => {
    expect(schemaValueFromWit(schemaValueToWit(v.f64(NaN)))).toEqual(v.f64(NaN));
    roundtripValue(v.f64(Infinity));
    roundtripValue(v.f64(-Infinity));
    roundtripValue(v.f32(Infinity));

    const negZero = schemaValueFromWit(schemaValueToWit(v.f64(-0)));
    expect(negZero.tag).toBe('f64');
    if (negZero.tag === 'f64') {
      expect(Object.is(negZero.value, -0)).toBe(true);
    }
  });

  it('non-BMP char round-trips', () => {
    roundtripValue(v.char('😀'));
  });
});

describe('rich semantic and capability values', () => {
  it('text/binary/path/url/datetime/duration round-trip', () => {
    roundtripValue({ tag: 'text', text: 'hello', language: 'en' });
    roundtripValue({ tag: 'text', text: 'no language' });
    roundtripValue({
      tag: 'binary',
      bytes: new Uint8Array([0, 1, 255]),
      mimeType: 'application/octet-stream',
    });
    roundtripValue({ tag: 'binary', bytes: new Uint8Array([]) });
    roundtripValue({ tag: 'path', value: '/tmp/x' });
    roundtripValue({ tag: 'url', value: 'https://example.com' });
    roundtripValue({ tag: 'datetime', value: { seconds: 1_700_000_000n, nanoseconds: 123 } });
    roundtripValue({ tag: 'duration', nanoseconds: -42n });
  });

  it('quantity fixed-point (including negative scale) round-trips', () => {
    roundtripValue({ tag: 'quantity', value: { mantissa: 12345n, scale: 3, unit: 'kg' } });
    roundtripValue({ tag: 'quantity', value: { mantissa: -98765n, scale: -3, unit: 'm' } });
  });

  it('union and secret values round-trip', () => {
    roundtripValue({ tag: 'union', unionTag: 'ssh', body: v.string('ssh://host') });
    roundtripValue({ tag: 'secret', secretRef: 'ref-abc' });
  });

  it('quota-token handle is lowered once and lifted back as an opaque handle', () => {
    // `own<quota-token>` is opaque; a plain sentinel object stands in for the
    // generated resource handle.
    const raw = { id: 'opaque-quota-token' } as never;
    const handle = GuestQuotaTokenHandle.fromRaw(raw);
    expect(handle.isPresent()).toBe(true);

    const wit = schemaValueToWit(v.quotaToken(handle));
    // Lowering moves the owned handle into a `quota-token-handle` wire node...
    expect(wit.valueNodes[wit.root]).toEqual({ tag: 'quota-token-handle', val: raw });
    // ...and consumes the source handle (affine: send-once).
    expect(handle.isPresent()).toBe(false);

    const decoded = schemaValueFromWit(wit);
    expect(decoded.tag).toBe('quota-token');
    if (decoded.tag === 'quota-token') {
      expect(decoded.handle.isPresent()).toBe(true);
      expect(decoded.handle.take()).toBe(raw);
    }
  });

  it('encoding an already-transferred quota-token handle is rejected', () => {
    const handle = GuestQuotaTokenHandle.fromRaw({} as never);
    schemaValueToWit(v.quotaToken(handle));
    expect(() => schemaValueToWit(v.quotaToken(handle))).toThrow(/already transferred/);
  });

  it('aliasing one quota-token handle twice in a tree is rejected without transferring it', () => {
    const handle = GuestQuotaTokenHandle.fromRaw({} as never);
    const aliased: SchemaValue = {
      tag: 'record',
      fields: [v.quotaToken(handle), v.quotaToken(handle)],
    };
    expect(() => schemaValueToWit(aliased)).toThrow(/more than once/);
    // The preflight rejects before any handle is moved out (atomic lowering).
    expect(handle.isPresent()).toBe(true);
  });

  it('decoding a tree with an unreferenced quota-token handle node is rejected', () => {
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'string-value', val: 'root' },
        // A handle node not reachable from the root must not be silently dropped.
        { tag: 'quota-token-handle', val: {} as never },
      ],
      root: 0,
    };
    expect(() => schemaValueFromWit(wit)).toThrow(/not referenced from the root/);
  });

  it('quota-token handles cannot be serialized to JSON', () => {
    const handle = GuestQuotaTokenHandle.fromRaw({} as never);
    expect(() => JSON.stringify(handle)).toThrow(/cannot be serialized/);
  });

  it('discriminated union types with every rule round-trip', () => {
    roundtripGraph({
      defs: new Map(),
      root: {
        body: {
          tag: 'union',
          branches: [
            {
              tag: 'ssh',
              body: t.string(),
              discriminator: { tag: 'prefix', val: 'ssh://' },
              metadata: { aliases: [], examples: [] },
            },
            {
              tag: 'tar',
              body: t.string(),
              discriminator: { tag: 'suffix', val: '.tar.gz' },
              metadata: { aliases: [], examples: [] },
            },
            {
              tag: 'has-colon',
              body: t.string(),
              discriminator: { tag: 'contains', val: ':' },
              metadata: { aliases: [], examples: [] },
            },
            {
              tag: 'rx',
              body: t.string(),
              discriminator: { tag: 'regex', val: '^a.*z$' },
              metadata: { aliases: [], examples: [] },
            },
            {
              tag: 'circle',
              body: t.record([field('kind', t.string())]),
              discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'circle' } },
              metadata: { aliases: [], examples: [] },
            },
            {
              tag: 'legacy',
              body: t.record([field('name', t.string())]),
              discriminator: { tag: 'field-absent', val: 'kind' },
              metadata: { aliases: [], examples: [] },
            },
          ],
        },
        metadata: { aliases: [], examples: [] },
      },
    });
  });
});

describe('flat-carrier DAG sharing (decode expands shared nodes)', () => {
  it('a value tree whose siblings share one child node index decodes correctly', () => {
    // node 0 is referenced by two record fields (a DAG, not a cycle).
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 's32-value', val: 7 },
        { tag: 'record-value', val: [0, 0] },
      ],
      root: 1,
    };
    const decoded = schemaValueFromWit(wit);
    expect(decoded).toEqual(v.record([v.s32(7), v.s32(7)]));
    // Re-encoding canonicalises (sharing is expanded), so the node count grows.
    const reencoded = schemaValueToWit(decoded);
    expect(reencoded.valueNodes.length).toBe(3);
  });

  it('a type graph whose siblings share one type node index decodes correctly', () => {
    const m = emptyMetadata();
    const wit: WitSchemaGraph = {
      typeNodes: [
        { body: { tag: 's32-type' }, metadata: m },
        {
          body: {
            tag: 'record-type',
            val: [
              { name: 'a', body: 0, metadata: m },
              { name: 'b', body: 0, metadata: m },
            ],
          },
          metadata: m,
        },
      ],
      defs: [],
      root: 1,
    };
    const decoded = schemaGraphFromWit(wit);
    expect(decoded.root).toEqual(t.record([field('a', t.s32()), field('b', t.s32())]));
  });
});

describe('metadata and roles', () => {
  it('metadata envelope (doc/aliases/examples/deprecated/role) round-trips', () => {
    const graph: SchemaGraph = {
      defs: new Map(),
      root: {
        body: { tag: 'record', fields: [field('x', t.s32())] },
        metadata: {
          doc: 'a record',
          aliases: ['rec', 'r'],
          examples: ['{"x":1}'],
          deprecated: 'use Y',
          role: { tag: 'multimodal' },
        },
      },
    };
    roundtripGraph(graph);

    const graph2: SchemaGraph = {
      defs: new Map(),
      root: {
        body: { tag: 'list', element: t.s32() },
        metadata: { aliases: [], examples: [], role: { tag: 'other', val: 'custom-role' } },
      },
    };
    roundtripGraph(graph2);
  });
});
