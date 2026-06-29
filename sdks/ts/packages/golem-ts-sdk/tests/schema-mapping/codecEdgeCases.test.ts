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
import { r, resolvedField } from '../../src/internal/mapping/types/resolvedType';
import { serialize, deserialize } from '../../src/internal/mapping/values/schemaValue';
import { Result } from '../../src/host/result';

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
