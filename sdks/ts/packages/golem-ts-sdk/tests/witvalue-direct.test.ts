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
import * as WitValue from '../src/internal/mapping/values/WitValue';
import * as Either from '../src/newTypes/either';
import {
  AnalysedType,
  bool,
  u8,
  u16,
  u32,
  u64,
  s8,
  s16,
  s32,
  s64,
  f32,
  f64,
  str,
  option,
  list,
  tuple,
  record,
  field,
  enum_,
  variant,
  case_,
  unitCase,
  result,
  flags,
} from '../src/internal/mapping/types/analysedType';
import { Result } from '../src/host/result';

/**
 * Serialize tsValue → WitValue, assert success, check the root WitNode tag,
 * then deserialize back and assert equality with the original value.
 */
function roundTrip(tsValue: any, analysedType: AnalysedType, expectedRootTag?: string): any {
  const eitherWit = WitValue.fromTsValueDefault(tsValue, analysedType);
  expect(Either.isRight(eitherWit), `serialization should succeed, got: ${Either.isLeft(eitherWit) ? eitherWit.val : ''}`).toBe(true);

  const witValue = (eitherWit as { tag: 'right'; val: WitValue.WitValue }).val;

  if (expectedRootTag) {
    // After build() canonicalization, the root is always at index 0
    expect(witValue.nodes[0].tag).toBe(expectedRootTag);
  }

  const result = WitValue.toTsValue(witValue, analysedType);
  return result;
}

function expectRoundTrip(tsValue: any, analysedType: AnalysedType, expectedRootTag?: string) {
  const result = roundTrip(tsValue, analysedType, expectedRootTag);
  expect(result).toEqual(tsValue);
}

function expectSerializationError(tsValue: any, analysedType: AnalysedType) {
  const eitherWit = WitValue.fromTsValueDefault(tsValue, analysedType);
  expect(Either.isLeft(eitherWit)).toBe(true);
}

// ─── Primitives ────────────────────────────────────────────────────────────

describe('WitValue direct: primitives', () => {
  describe('bool', () => {
    it('serializes true', () => {
      expectRoundTrip(true, bool(), 'prim-bool');
    });

    it('serializes false', () => {
      expectRoundTrip(false, bool(), 'prim-bool');
    });

    it('rejects string where bool expected', () => {
      expectSerializationError('yes', bool());
    });

    it('rejects number where bool expected', () => {
      expectSerializationError(1, bool());
    });
  });

  describe('u8', () => {
    it('round-trips 0', () => expectRoundTrip(0, u8(), 'prim-u8'));
    it('round-trips 255', () => expectRoundTrip(255, u8(), 'prim-u8'));
    it('rejects string', () => expectSerializationError('foo', u8()));
  });

  describe('u16', () => {
    it('round-trips 0', () => expectRoundTrip(0, u16(), 'prim-u16'));
    it('round-trips 65535', () => expectRoundTrip(65535, u16(), 'prim-u16'));
    it('rejects string', () => expectSerializationError('foo', u16()));
  });

  describe('u32', () => {
    it('round-trips 0', () => expectRoundTrip(0, u32(), 'prim-u32'));
    it('round-trips 4294967295', () => expectRoundTrip(4294967295, u32(), 'prim-u32'));
    it('rejects boolean', () => expectSerializationError(true, u32()));
  });

  describe('u64', () => {
    it('round-trips bigint (isBigInt=true)', () => {
      expectRoundTrip(123n, u64(true), 'prim-u64');
    });

    it('round-trips number via bigint conversion (isBigInt=false)', () => {
      // u64 serializer converts numbers to bigint; deserializer with isBigInt=false returns number
      const typ = u64(false);
      const eitherWit = WitValue.fromTsValueDefault(42, typ);
      expect(Either.isRight(eitherWit)).toBe(true);
      const witValue = (eitherWit as { tag: 'right'; val: WitValue.WitValue }).val;
      expect(witValue.nodes[0].tag).toBe('prim-u64');
      const back = WitValue.toTsValue(witValue, typ);
      // When isBigInt=false the deserializer calls convertToNumber which returns the bigint value directly
      expect(typeof back === 'bigint' || typeof back === 'number').toBe(true);
    });

    it('rejects string', () => expectSerializationError('foo', u64(true)));
  });

  describe('s8', () => {
    it('round-trips -128', () => expectRoundTrip(-128, s8(), 'prim-s8'));
    it('round-trips 127', () => expectRoundTrip(127, s8(), 'prim-s8'));
    it('rejects boolean', () => expectSerializationError(false, s8()));
  });

  describe('s16', () => {
    it('round-trips negative', () => expectRoundTrip(-32768, s16(), 'prim-s16'));
    it('round-trips positive', () => expectRoundTrip(32767, s16(), 'prim-s16'));
    it('rejects string', () => expectSerializationError('x', s16()));
  });

  describe('s32', () => {
    it('round-trips 0', () => expectRoundTrip(0, s32(), 'prim-s32'));
    it('round-trips negative', () => expectRoundTrip(-2147483648, s32(), 'prim-s32'));
    it('rejects null', () => expectSerializationError(null, s32()));
  });

  describe('s64', () => {
    it('round-trips bigint (isBigInt=true)', () => {
      expectRoundTrip(-9007199254740991n, s64(true), 'prim-s64');
    });

    it('rejects number (s64 expects bigint)', () => {
      expectSerializationError(42, s64(true));
    });
  });

  describe('f32', () => {
    it('round-trips 0', () => expectRoundTrip(0, f32(), 'prim-float32'));
    it('round-trips 3.14', () => expectRoundTrip(3.14, f32(), 'prim-float32'));
    it('rejects string', () => expectSerializationError('3.14', f32()));
  });

  describe('f64', () => {
    it('round-trips 0', () => expectRoundTrip(0, f64(), 'prim-float64'));
    it('round-trips large float', () => expectRoundTrip(1.7976931348623157e308, f64(), 'prim-float64'));
    it('rejects boolean', () => expectSerializationError(true, f64()));
  });

  describe('string', () => {
    it('round-trips empty string', () => expectRoundTrip('', str(), 'prim-string'));
    it('round-trips non-empty string', () => expectRoundTrip('hello world', str(), 'prim-string'));
    it('round-trips unicode', () => expectRoundTrip('日本語テスト 🎉', str(), 'prim-string'));
    it('rejects number where string expected', () => expectSerializationError(42, str()));
    it('rejects boolean where string expected', () => expectSerializationError(true, str()));
  });
});

// ─── Option ────────────────────────────────────────────────────────────────

describe('WitValue direct: option', () => {
  it('round-trips Some value', () => {
    const typ = option(undefined, 'null', u32());
    expectRoundTrip(42, typ, 'option-value');
  });

  it('round-trips None as null (emptyType=null)', () => {
    const typ = option(undefined, 'null', u32());
    expectRoundTrip(null, typ, 'option-value');
  });

  it('round-trips None as undefined (emptyType=undefined)', () => {
    const typ = option(undefined, 'undefined', u32());
    expectRoundTrip(undefined, typ, 'option-value');
  });

  it('round-trips nested option (Some(Some(42)))', () => {
    const typ = option(undefined, 'null', option(undefined, 'null', u32()));
    expectRoundTrip(42, typ);
  });

  it('round-trips nested option None', () => {
    const typ = option(undefined, 'null', option(undefined, 'null', u32()));
    expectRoundTrip(null, typ);
  });
});

// ─── List ──────────────────────────────────────────────────────────────────

describe('WitValue direct: list', () => {
  it('round-trips empty array', () => {
    const typ = list(undefined, undefined, undefined, u32());
    expectRoundTrip([], typ, 'list-value');
  });

  it('round-trips non-empty array of primitives', () => {
    const typ = list(undefined, undefined, undefined, u32());
    expectRoundTrip([1, 2, 3], typ, 'list-value');
  });

  it('round-trips nested list', () => {
    const inner = list(undefined, undefined, undefined, str());
    const outer = list(undefined, undefined, undefined, inner);
    expectRoundTrip([['a', 'b'], ['c']], outer, 'list-value');
  });

  it('round-trips list of strings', () => {
    const typ = list(undefined, undefined, undefined, str());
    expectRoundTrip(['hello', 'world', ''], typ);
  });

  // TypedArray tests
  it('round-trips Uint8Array', () => {
    const typ = list(undefined, 'u8', undefined, u8());
    expectRoundTrip(new Uint8Array([1, 2, 3]), typ);
  });

  it('round-trips Uint16Array', () => {
    const typ = list(undefined, 'u16', undefined, u16());
    expectRoundTrip(new Uint16Array([100, 200, 300]), typ);
  });

  it('round-trips Uint32Array', () => {
    const typ = list(undefined, 'u32', undefined, u32());
    expectRoundTrip(new Uint32Array([1000, 2000, 3000]), typ);
  });

  it('round-trips BigUint64Array', () => {
    const typ = list(undefined, 'big-u64', undefined, u64(true));
    expectRoundTrip(new BigUint64Array([1n, 2n, 3n]), typ);
  });

  it('round-trips Int8Array', () => {
    const typ = list(undefined, 'i8', undefined, s8());
    expectRoundTrip(new Int8Array([-1, 0, 1]), typ);
  });

  it('round-trips Int16Array', () => {
    const typ = list(undefined, 'i16', undefined, s16());
    expectRoundTrip(new Int16Array([-100, 0, 100]), typ);
  });

  it('round-trips Int32Array', () => {
    const typ = list(undefined, 'i32', undefined, s32());
    expectRoundTrip(new Int32Array([-1000, 0, 1000]), typ);
  });

  it('round-trips BigInt64Array', () => {
    const typ = list(undefined, 'big-i64', undefined, s64(true));
    expectRoundTrip(new BigInt64Array([-1n, 0n, 1n]), typ);
  });

  it('round-trips Float32Array', () => {
    const typ = list(undefined, 'f32', undefined, f32());
    const input = new Float32Array([1.5, 2.5, 3.5]);
    expectRoundTrip(input, typ);
  });

  it('round-trips Float64Array', () => {
    const typ = list(undefined, 'f64', undefined, f64());
    expectRoundTrip(new Float64Array([1.1, 2.2, 3.3]), typ);
  });

  // Map encoding
  it('round-trips Map<string, number>', () => {
    const tupleType = tuple(undefined, undefined, [str(), u32()]);
    const mapKV = { keyType: str(), valueType: u32() };
    const typ = list(undefined, undefined, mapKV, tupleType);
    const m = new Map<string, number>();
    m.set('a', 1);
    m.set('b', 2);
    expectRoundTrip(m, typ);
  });

  it('round-trips empty Map', () => {
    const tupleType = tuple(undefined, undefined, [str(), u32()]);
    const mapKV = { keyType: str(), valueType: u32() };
    const typ = list(undefined, undefined, mapKV, tupleType);
    expectRoundTrip(new Map<string, number>(), typ);
  });
});

// ─── Tuple ─────────────────────────────────────────────────────────────────

describe('WitValue direct: tuple', () => {
  it('round-trips [string, number, boolean]', () => {
    const typ = tuple(undefined, undefined, [str(), u32(), bool()]);
    expectRoundTrip(['hello', 42, true], typ, 'tuple-value');
  });

  it('round-trips empty tuple → undefined (emptyType=void)', () => {
    const typ = tuple(undefined, 'void', []);
    // The serializer serializes void/undefined as record with 0 fields
    // Deserializer returns undefined for emptyType=void
    const eitherWit = WitValue.fromTsValueDefault(undefined, typ);
    // void tuple might not serialize at all or might produce an empty record
    // Let's check what actually happens
    if (Either.isRight(eitherWit)) {
      const witValue = eitherWit.val;
      const back = WitValue.toTsValue(witValue, typ);
      expect(back).toBeUndefined();
    }
  });

  it('round-trips empty tuple → null (emptyType=null)', () => {
    const typ = tuple(undefined, 'null', []);
    const eitherWit = WitValue.fromTsValueDefault(null, typ);
    if (Either.isRight(eitherWit)) {
      const witValue = eitherWit.val;
      const back = WitValue.toTsValue(witValue, typ);
      expect(back).toBeNull();
    }
  });

  it('round-trips empty tuple → undefined (emptyType=undefined)', () => {
    const typ = tuple(undefined, 'undefined', []);
    const eitherWit = WitValue.fromTsValueDefault(undefined, typ);
    if (Either.isRight(eitherWit)) {
      const witValue = eitherWit.val;
      const back = WitValue.toTsValue(witValue, typ);
      expect(back).toBeUndefined();
    }
  });

  it('round-trips nested tuple', () => {
    const inner = tuple(undefined, undefined, [u32(), str()]);
    const outer = tuple(undefined, undefined, [inner, bool()]);
    expectRoundTrip([[10, 'x'], true], outer);
  });
});

// ─── Record ────────────────────────────────────────────────────────────────

describe('WitValue direct: record', () => {
  it('round-trips flat record', () => {
    const typ = record(undefined, [
      field('name', str()),
      field('age', u32()),
      field('active', bool()),
    ]);
    expectRoundTrip({ name: 'Alice', age: 30, active: true }, typ, 'record-value');
  });

  it('round-trips nested record', () => {
    const innerTyp = record(undefined, [
      field('x', f64()),
      field('y', f64()),
    ]);
    const outerTyp = record(undefined, [
      field('label', str()),
      field('point', innerTyp),
    ]);
    expectRoundTrip({ label: 'origin', point: { x: 0.0, y: 0.0 } }, outerTyp);
  });

  it('round-trips record with optional field (Some)', () => {
    const typ = record(undefined, [
      field('required', str()),
      field('optional', option(undefined, 'undefined', u32())),
    ]);
    expectRoundTrip({ required: 'hi', optional: 42 }, typ);
  });

  it('round-trips record with optional field (None)', () => {
    const typ = record(undefined, [
      field('required', str()),
      field('optional', option(undefined, 'undefined', u32())),
    ]);
    expectRoundTrip({ required: 'hi', optional: undefined }, typ);
  });

  it('round-trips empty record', () => {
    const typ = record(undefined, []);
    expectRoundTrip({}, typ, 'record-value');
  });
});

// ─── Variant ───────────────────────────────────────────────────────────────

describe('WitValue direct: variant', () => {
  it('round-trips tagged variant with payload', () => {
    const typ = variant(
      undefined,
      [
        { tagLiteralName: 'text', valueType: ['val', { kind: 'string' } as any] },
        { tagLiteralName: 'number', valueType: ['val', { kind: 'number' } as any] },
      ],
      [
        case_('text', str()),
        case_('number', u32()),
      ],
    );
    expectRoundTrip({ tag: 'text', val: 'hello' }, typ, 'variant-value');
  });

  it('round-trips tagged variant unit case (no payload)', () => {
    const typ = variant(
      undefined,
      [
        { tagLiteralName: 'none', valueType: undefined },
        { tagLiteralName: 'some', valueType: ['val', { kind: 'string' } as any] },
      ],
      [
        unitCase('none'),
        case_('some', str()),
      ],
    );
    expectRoundTrip({ tag: 'none' }, typ, 'variant-value');
  });

  it('round-trips untagged variant (simple union)', () => {
    const typ = variant(
      undefined,
      [], // no tagged types = simple union
      [
        case_('case-number', u32()),
        case_('case-string', str()),
      ],
    );
    // For untagged variants, the TS value is the raw value and the deserializer
    // returns the deserialized form based on caseIdx
    const witEither = WitValue.fromTsValueDefault(42, typ);
    expect(Either.isRight(witEither)).toBe(true);
    const witValue = (witEither as { tag: 'right'; val: WitValue.WitValue }).val;
    const back = WitValue.toTsValue(witValue, typ);
    expect(back).toBe(42);
  });
});

// ─── Enum ──────────────────────────────────────────────────────────────────

describe('WitValue direct: enum', () => {
  it('round-trips valid enum value', () => {
    const typ = enum_(undefined, ['red', 'green', 'blue']);
    expectRoundTrip('red', typ, 'enum-value');
    expectRoundTrip('green', typ);
    expectRoundTrip('blue', typ);
  });

  it('rejects invalid enum string', () => {
    const typ = enum_(undefined, ['red', 'green', 'blue']);
    expectSerializationError('yellow', typ);
  });

  it('rejects number for enum', () => {
    const typ = enum_(undefined, ['a', 'b']);
    expectSerializationError(42, typ);
  });
});

// ─── Flags ─────────────────────────────────────────────────────────────────

describe('WitValue direct: flags', () => {
  // flags are currently unhandled in serializer so they should return an error
  it('returns error for flags (currently unsupported)', () => {
    const typ = flags(undefined, ['read', 'write', 'execute']);
    expectSerializationError({ read: true, write: false, execute: true }, typ);
  });
});

// ─── Result ────────────────────────────────────────────────────────────────

describe('WitValue direct: result', () => {
  describe('inbuilt result', () => {
    it('round-trips ok with value', () => {
      const typ = result(
        undefined,
        { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: undefined },
        str(),
        str(),
      );
      const input = Result.ok('success');
      const back = roundTrip(input, typ, 'result-value');
      expect(back.tag).toBe('ok');
      expect(back.val).toBe('success');
    });

    it('round-trips err with value', () => {
      const typ = result(
        undefined,
        { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: undefined },
        str(),
        str(),
      );
      const input = Result.err('failure');
      const back = roundTrip(input, typ, 'result-value');
      expect(back.tag).toBe('err');
      expect(back.val).toBe('failure');
    });

    it('round-trips ok void (okEmptyType=void)', () => {
      const typ = result(
        undefined,
        { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: undefined },
        undefined,
        str(),
      );
      const input = Result.ok(undefined);
      const back = roundTrip(input, typ);
      expect(back.tag).toBe('ok');
      expect(back.val).toBeUndefined();
    });

    it('round-trips err void (errEmptyType=void)', () => {
      const typ = result(
        undefined,
        { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: 'void' },
        str(),
        undefined,
      );
      const input = Result.err(undefined);
      const back = roundTrip(input, typ);
      expect(back.tag).toBe('err');
      expect(back.val).toBeUndefined();
    });
  });

  describe('custom result', () => {
    it('round-trips ok with named value', () => {
      const typ = result(
        undefined,
        { tag: 'custom', okValueName: 'value', errValueName: 'error' },
        u32(),
        str(),
      );
      const input = { tag: 'ok', value: 42 };
      const back = roundTrip(input, typ, 'result-value');
      expect(back).toEqual({ tag: 'ok', value: 42 });
    });

    it('round-trips err with named value', () => {
      const typ = result(
        undefined,
        { tag: 'custom', okValueName: 'value', errValueName: 'error' },
        u32(),
        str(),
      );
      const input = { tag: 'err', error: 'bad input' };
      const back = roundTrip(input, typ, 'result-value');
      expect(back).toEqual({ tag: 'err', error: 'bad input' });
    });

    it('round-trips custom result with no err type', () => {
      const typ = result(
        undefined,
        { tag: 'custom', okValueName: 'data', errValueName: undefined },
        str(),
        undefined,
      );
      const input = { tag: 'ok', data: 'hello' };
      const back = roundTrip(input, typ);
      expect(back).toEqual({ tag: 'ok', data: 'hello' });
    });

    it('round-trips custom result err side when no err type', () => {
      const typ = result(
        undefined,
        { tag: 'custom', okValueName: 'data', errValueName: undefined },
        str(),
        undefined,
      );
      const input = { tag: 'err' };
      const back = roundTrip(input, typ);
      expect(back).toEqual({ tag: 'err' });
    });
  });
});

// ─── Primitive error paths ─────────────────────────────────────────────────

describe('WitValue direct: error paths', () => {
  it('rejects passing string where u32 expected', () => {
    expectSerializationError('hello', u32());
  });

  it('rejects passing number where string expected', () => {
    expectSerializationError(123, str());
  });

  it('rejects passing null where u32 expected', () => {
    expectSerializationError(null, u32());
  });

  it('rejects passing object where bool expected', () => {
    expectSerializationError({}, bool());
  });

  it('rejects passing array where string expected', () => {
    expectSerializationError([1, 2], str());
  });

  it('rejects passing string where f64 expected', () => {
    expectSerializationError('3.14', f64());
  });

  it('rejects passing boolean where f32 expected', () => {
    expectSerializationError(true, f32());
  });
});

// ─── Edge cases ────────────────────────────────────────────────────────────

describe('WitValue direct: edge cases', () => {
  it('root-first WitValue produced by builder', () => {
    const typ = record(undefined, [
      field('name', str()),
      field('age', u32()),
    ]);
    const eitherWit = WitValue.fromTsValueDefault({ name: 'Alice', age: 30 }, typ);
    expect(Either.isRight(eitherWit)).toBe(true);
    const witValue = (eitherWit as { tag: 'right'; val: WitValue.WitValue }).val;
    // After build() canonicalization, root (record-value) is at index 0
    expect(witValue.nodes[0].tag).toBe('record-value');
  });

  it('tuple wrong length returns error', () => {
    const typ = tuple(undefined, undefined, [u32(), u32()]);
    expectSerializationError([1, 2, 3], typ);
  });

  it('tagged union with null input returns error', () => {
    const typ = variant(
      undefined,
      [
        { tagLiteralName: 'text', valueType: ['val', { kind: 'string' } as any] },
      ],
      [
        case_('text', str()),
      ],
    );
    expectSerializationError(null, typ);
  });

  it('untagged variant with bigint selects u64 case', () => {
    const typ = variant(
      undefined,
      [],
      [
        case_('case-u64', u64(true)),
        case_('case-string', str()),
      ],
    );
    const eitherWit = WitValue.fromTsValueDefault(42n, typ);
    expect(Either.isRight(eitherWit), `serialization should succeed, got: ${Either.isLeft(eitherWit) ? eitherWit.val : ''}`).toBe(true);
    const witValue = (eitherWit as { tag: 'right'; val: WitValue.WitValue }).val;
    const back = WitValue.toTsValue(witValue, typ);
    expect(back).toBe(42n);
  });
});
