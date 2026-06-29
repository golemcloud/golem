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

// Slice 2: focused parity / behaviour checks for the value codec on manually
// constructed `ResolvedType`s (independent of the legacy mapper). These pin the
// parity edge cases raised in the oracle review of Slice 2.

import { describe, it, expect } from 'vitest';
import type { SchemaValueNode as WitSchemaValueNode } from 'golem:core/types@2.0.0';
import { r, resolvedField } from '../../src/internal/mapping/types/resolvedType';
import {
  createWireDecoder,
  compileGraphDecoder,
  getGraphCodec,
  serialize,
  deserialize,
  deserializeGraph,
  deserializeGraphFromWit,
} from '../../src/internal/mapping/values/schemaValue';
import { Result } from '../../src/host/result';
import { GuestSecretHandle } from '../../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../../src/internal/schema-model/secretInternal';

describe('Slice 2 value codec — result parity', () => {
  const resultType = r.result(r.f64(), r.string(), { tag: 'inbuilt' });

  it('inbuilt result requires a `val` key (legacy parity)', () => {
    expect(() => serialize({ tag: 'ok' }, resultType)).toThrow(/Missing key 'val'/);
    expect(() => serialize({ tag: 'err' }, resultType)).toThrow(/Missing key 'val'/);
  });

  it('inbuilt result round-trips ok/err with payload', () => {
    const ok = serialize({ tag: 'ok', val: 3 }, resultType);
    expect(deserialize(ok, resultType)).toEqual(Result.ok(3));
    const err = serialize({ tag: 'err', val: 'boom' }, resultType);
    expect(deserialize(err, resultType)).toEqual(Result.err('boom'));
  });

  it('inbuilt Result<Option<T>, E> round-trips a present option payload', () => {
    const t = r.result(r.option(r.f64(), 'undefined'), r.string(), { tag: 'inbuilt' });
    const sv = serialize({ tag: 'ok', val: 5 }, t);
    expect(deserialize(sv, t)).toEqual(Result.ok(5));
    const svNone = serialize({ tag: 'ok', val: undefined }, t);
    expect(deserialize(svNone, t)).toEqual(Result.ok(undefined));
  });
});

describe('Slice 2 value codec — map strictness', () => {
  const mapType = r.map(r.string(), r.f64());

  it('round-trips a Map value', () => {
    const sv = serialize(
      new Map([
        ['a', 1],
        ['b', 2],
      ]),
      mapType,
    );
    expect(deserialize(sv, mapType)).toEqual(
      new Map([
        ['a', 1],
        ['b', 2],
      ]),
    );
  });

  it('rejects an array-of-tuples for a map type (stricter than legacy, documented)', () => {
    expect(() => serialize([['a', 1]], mapType)).toThrow(/Type mismatch/);
  });
});

describe('Slice 2 value codec — plain union with tagged-variant case', () => {
  // A plain union whose only payload case is itself a tagged variant. A value
  // with a non-string `tag` must not match the tagged case and must fall
  // through to plain matching (here: no match -> union mismatch).
  const innerTagged = r.variant(true, [{ name: 'a', payload: r.f64(), valueKey: 'val' }]);
  const plain = r.variant(false, [{ name: 'lit' }, { name: 'AnonField1', payload: innerTagged }]);

  it('matches the literal case', () => {
    const sv = serialize('lit', plain);
    expect(deserialize(sv, plain)).toEqual('lit');
  });

  it('matches the tagged-variant payload case via structural match', () => {
    const sv = serialize({ tag: 'a', val: 2 }, plain);
    expect(deserialize(sv, plain)).toEqual({ tag: 'a', val: 2 });
  });

  it('a non-string tag does not spuriously match the tagged case', () => {
    expect(() => serialize({ tag: 123, val: 2 }, plain)).toThrow(/does not match any/);
  });
});

describe('Slice 2 value codec — record optional fields', () => {
  const rec = r.record([
    resolvedField('a', r.string()),
    resolvedField('b', r.option(r.f64(), 'undefined')),
  ]);

  it('serializes an object omitting an optional field', () => {
    const sv = serialize({ a: 'x' }, rec);
    expect(deserialize(sv, rec)).toEqual({ a: 'x', b: undefined });
  });

  it('serializes an object including the optional field', () => {
    const sv = serialize({ a: 'x', b: 7 }, rec);
    expect(deserialize(sv, rec)).toEqual({ a: 'x', b: 7 });
  });
});

describe('Slice 2 value codec — reusable wire decoder', () => {
  it('review repro: a failed readGraph call does not poison later reads with a stale cycle marker', () => {
    const nodes = [
      { tag: 'record-value', val: [1] },
      { tag: 'string-value', val: 'not-a-u32' },
    ] as Parameters<typeof createWireDecoder>[0];
    const graph = { defs: new Map(), root: r.record([resolvedField('x', r.u32())]) };
    const decoder = createWireDecoder(nodes);

    expect(() => decoder.readGraph(0, graph)).toThrow(/number/);
    expect(() => decoder.readGraph(0, graph)).toThrow(/number/);
  });

  it('review repro: compiled codec read failure does not poison the shared cycle guard', () => {
    const nodes = [
      { tag: 'record-value', val: [1] },
      { tag: 'string-value', val: 'not-a-u32' },
    ] as Parameters<typeof createWireDecoder>[0];
    const graph = { defs: new Map(), root: r.record([resolvedField('x', r.u32())]) };
    const codec = getGraphCodec(graph);
    expect(codec).not.toBeNull();

    const onPath = new Uint8Array(nodes.length);

    expect(() => codec!.read(0, nodes, onPath)).toThrow(/number/);
    expect(onPath[0]).toBe(0);
    expect(() => codec!.read(0, nodes, onPath)).toThrow(/number/);
  });
});

describe('Slice 2 value codec — strict enum decode', () => {
  it('review repro: out-of-range enum indices are rejected on all schema decode paths', () => {
    const graph = { defs: new Map(), root: r.enum(['red']) };
    const wit = { valueNodes: [{ tag: 'enum-value', val: 1 }], root: 0 } as Parameters<
      typeof deserializeGraphFromWit
    >[0];

    const rejects = (f: () => unknown) => {
      try {
        f();
        return false;
      } catch {
        return true;
      }
    };

    expect({
      schemaValue: rejects(() => deserializeGraph({ tag: 'enum', caseIndex: 1 }, graph)),
      wireValue: rejects(() => deserializeGraphFromWit(wit, graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit)),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });

  it('review repro: non-integer enum indices are rejected on all schema decode paths', () => {
    const graph = { defs: new Map(), root: r.enum(['red', 'blue']) };
    const wit = { valueNodes: [{ tag: 'enum-value', val: 0.5 }], root: 0 } as Parameters<
      typeof deserializeGraphFromWit
    >[0];

    const rejects = (f: () => unknown) => {
      try {
        f();
        return false;
      } catch {
        return true;
      }
    };

    expect({
      schemaValue: rejects(() => deserializeGraph({ tag: 'enum', caseIndex: 0.5 }, graph)),
      wireValue: rejects(() => deserializeGraphFromWit(wit, graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit)),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });
});

describe('Slice 2 value codec — strict variant decode', () => {
  it('review repro: option-payload variant cases reject a missing variant payload', () => {
    const graph = {
      defs: new Map(),
      root: r.variant(true, [
        { name: 'maybe', payload: r.option(r.u32(), 'undefined'), valueKey: 'value' },
      ]),
    };
    const wit = {
      valueNodes: [{ tag: 'variant-value', val: { case_: 0, payload: undefined } }],
      root: 0,
    } as Parameters<typeof deserializeGraphFromWit>[0];

    const rejects = (f: () => unknown) => {
      try {
        f();
        return false;
      } catch {
        return true;
      }
    };

    expect({
      schemaValue: rejects(() => deserializeGraph({ tag: 'variant', caseIndex: 0 }, graph)),
      wireValue: rejects(() => deserializeGraphFromWit(wit, graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit)),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });
});

describe('Slice 2 value codec — strict custom result decode', () => {
  const graph = {
    defs: new Map(),
    root: r.result(r.u32(), undefined, {
      tag: 'custom' as const,
      okValueName: 'value',
      errValueName: 'error',
    }),
  };

  const rejects = (f: () => unknown) => {
    try {
      f();
      return false;
    } catch {
      return true;
    }
  };

  it('review repro: absent custom result side rejects unexpected payloads carrying owned secret handles', () => {
    const raw = { id: 'ignored-secret' } as never;
    const wit = () => ({
      valueNodes: [
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
        { tag: 'result-value', val: { tag: 'err-value', val: 0 } } as WitSchemaValueNode,
      ],
      root: 1,
    });

    expect({
      schemaValue: rejects(() =>
        deserializeGraph(
          {
            tag: 'result',
            result: {
              tag: 'err',
              value: {
                tag: 'secret',
                handle: GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw),
              },
            },
          },
          graph,
        ),
      ),
      wireValue: rejects(() => deserializeGraphFromWit(wit(), graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit())),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });

  it('review repro: present custom result side rejects missing payloads', () => {
    const wit = {
      valueNodes: [
        { tag: 'result-value', val: { tag: 'ok-value', val: undefined } } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect({
      schemaValue: rejects(() =>
        deserializeGraph({ tag: 'result', result: { tag: 'ok', value: undefined } }, graph),
      ),
      wireValue: rejects(() => deserializeGraphFromWit(wit, graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit)),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });
});
