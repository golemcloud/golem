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

// Runtime value codec: TypeScript values <-> the recursive `SchemaValue` model,
// guided by a `ResolvedType`. This replaces the legacy `WitValue`/`WitNode`
// serializer/deserializer. The flat WIT `schema-value-tree` carrier is produced
// from / consumed by `SchemaValue` via the schema-model codecs
// (`schemaValueToWit` / `schemaValueFromWit`).
//
// Numeric handling is JS-type-guard only (parity with the legacy SDK): no scalar
// range checks and no `Math.fround`. 64-bit integers are always `bigint`.
//
// The codec is graph-aware: a `Defs` registry (the `defs` of a `ResolvedGraph`)
// is threaded through every recursive call so that a `ref` body — the back-edge
// used to close recursive / mutually-recursive types — is resolved to its named
// composite body before dispatching. Finite values terminate naturally even
// through a recursive type. Non-recursive callers can omit `defs` (defaults to
// empty); a `ResolvedType` lifted from the legacy analysed model never contains
// refs, so those call sites are unaffected.

import * as util from 'node:util';
import {
  ResolvedType,
  ResolvedField,
  ResolvedVariantCase,
  ResolvedGraph,
  TypedArrayKind,
  TypeId,
} from '../types/resolvedType';
import { SchemaValue, v } from '../../schema-model';
import { QUOTA_INTERNAL } from '../../schema-model/quotaInternal';
import { Result } from '../../../host/result';
import { QuotaToken } from '../../../host/quota';

// ============================================================
// Errors
// ============================================================

function display(value: unknown): string {
  return util.format(value);
}

function typeMismatch(value: unknown, expected: string): Error {
  return new Error(`Type mismatch. Expected \`${expected}\`, got \`${display(value)}\``);
}

function deserializeMismatch(value: SchemaValue, expected: string): Error {
  return new Error(
    `Failed to deserialize schema value with tag \`${value.tag}\` to TypeScript type \`${expected}\``,
  );
}

function missingKey(key: string, value: unknown): Error {
  return new Error(`Missing key '${key}' in ${display(value)}`);
}

function unionMismatch(cases: ResolvedVariantCase[], value: unknown): Error {
  return new Error(
    `Value '${display(value)}' does not match any of the union cases: ${cases.map((c) => c.name).join(', ')}`,
  );
}

function internalError(message: string): Error {
  return new Error(`Internal error: ${message}`);
}

// ============================================================
// Ref resolution
// ============================================================

/** A registry of named composite definitions, keyed by stable `type-id`. */
export type Defs = ReadonlyMap<TypeId, ResolvedType>;

const EMPTY_DEFS: Defs = new Map();

/** Follow `ref` bodies through `defs` until a concrete (non-ref) type is reached. */
function resolveRef(rt: ResolvedType, defs: Defs): ResolvedType {
  let current = rt;
  const seen = new Set<TypeId>();
  while (current.body.tag === 'ref') {
    const id = current.body.id;
    if (seen.has(id)) throw internalError(`cyclic type ref chain at '${id}'`);
    seen.add(id);
    const target = defs.get(id);
    if (!target) throw internalError(`unresolved type ref '${id}'`);
    current = target;
  }
  return current;
}

// ============================================================
// Typed-array support
// ============================================================

type TypedArrayCtor = {
  is: (x: unknown) => boolean;
  name: string;
  make: (len: number) => { [index: number]: number | bigint; length: number };
};

const TYPED_ARRAYS: Record<TypedArrayKind, TypedArrayCtor> = {
  u8: { is: (x) => x instanceof Uint8Array, name: 'Uint8Array', make: (n) => new Uint8Array(n) },
  u16: {
    is: (x) => x instanceof Uint16Array,
    name: 'Uint16Array',
    make: (n) => new Uint16Array(n),
  },
  u32: {
    is: (x) => x instanceof Uint32Array,
    name: 'Uint32Array',
    make: (n) => new Uint32Array(n),
  },
  'big-u64': {
    is: (x) => x instanceof BigUint64Array,
    name: 'BigUint64Array',
    make: (n) => new BigUint64Array(n),
  },
  i8: { is: (x) => x instanceof Int8Array, name: 'Int8Array', make: (n) => new Int8Array(n) },
  i16: { is: (x) => x instanceof Int16Array, name: 'Int16Array', make: (n) => new Int16Array(n) },
  i32: { is: (x) => x instanceof Int32Array, name: 'Int32Array', make: (n) => new Int32Array(n) },
  'big-i64': {
    is: (x) => x instanceof BigInt64Array,
    name: 'BigInt64Array',
    make: (n) => new BigInt64Array(n),
  },
  f32: {
    is: (x) => x instanceof Float32Array,
    name: 'Float32Array',
    make: (n) => new Float32Array(n),
  },
  f64: {
    is: (x) => x instanceof Float64Array,
    name: 'Float64Array',
    make: (n) => new Float64Array(n),
  },
};

// ============================================================
// Graph-aware entry points
// ============================================================

/** Serialize a TS value against a `ResolvedGraph` root (recursion-aware). */
export function serializeGraph(tsValue: any, graph: ResolvedGraph): SchemaValue {
  return serialize(tsValue, graph.root, graph.defs);
}

/** Deserialize a `SchemaValue` against a `ResolvedGraph` root (recursion-aware). */
export function deserializeGraph(value: SchemaValue, graph: ResolvedGraph): any {
  return deserialize(value, graph.root, graph.defs);
}

/** Structural match of a TS value against a `ResolvedGraph` root (recursion-aware). */
export function matchesResolvedGraph(value: any, graph: ResolvedGraph): boolean {
  return matchesResolved(value, graph.root, graph.defs);
}

// ============================================================
// Serialization: TS value -> SchemaValue
// ============================================================

export function serialize(tsValue: any, rt: ResolvedType, defs: Defs = EMPTY_DEFS): SchemaValue {
  const b = resolveRef(rt, defs).body;
  switch (b.tag) {
    case 'bool':
      if (typeof tsValue !== 'boolean') throw typeMismatch(tsValue, 'boolean');
      return v.bool(tsValue);

    case 'f32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.f32(tsValue);
    case 'f64':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.f64(tsValue);
    case 'u8':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.u8(tsValue);
    case 'u16':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.u16(tsValue);
    case 'u32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.u32(tsValue);
    case 's8':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.s8(tsValue);
    case 's16':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.s16(tsValue);
    case 's32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return v.s32(tsValue);

    case 'u64':
      if (typeof tsValue === 'bigint') return v.u64(tsValue);
      if (typeof tsValue === 'number') return v.u64(BigInt(tsValue));
      throw typeMismatch(tsValue, 'bigint');
    case 's64':
      if (typeof tsValue !== 'bigint') throw typeMismatch(tsValue, 'bigint');
      return v.s64(tsValue);

    case 'char':
      if (typeof tsValue !== 'string') throw typeMismatch(tsValue, 'string');
      return v.char(tsValue);
    case 'string':
      if (typeof tsValue !== 'string') throw typeMismatch(tsValue, 'string');
      return v.string(tsValue);

    case 'option':
      if (tsValue === null || tsValue === undefined) return v.option(undefined);
      return v.option(serialize(tsValue, b.element, defs));

    case 'list':
      return serializeList(tsValue, b.element, b.typedArray, defs);

    case 'map': {
      if (!(tsValue instanceof Map)) throw typeMismatch(tsValue, 'Map');
      const entries = Array.from(tsValue.entries()).map(([k, val]) => ({
        key: serialize(k, b.key, defs),
        value: serialize(val, b.value, defs),
      }));
      return v.map(entries);
    }

    case 'tuple': {
      if (b.empty !== undefined) {
        if (tsValue === null || tsValue === undefined) return v.tuple([]);
        throw typeMismatch(tsValue, 'empty tuple');
      }
      if (!Array.isArray(tsValue) || tsValue.length !== b.elements.length) {
        throw typeMismatch(tsValue, `Array of length ${b.elements.length}`);
      }
      return v.tuple(b.elements.map((et, i) => serialize(tsValue[i], et, defs)));
    }

    case 'record':
      return serializeRecord(tsValue, b.fields, defs);

    case 'variant':
      return serializeVariant(tsValue, b, defs);

    case 'enum': {
      if (typeof tsValue === 'string') {
        const idx = b.cases.indexOf(tsValue);
        if (idx !== -1) return v.enum(idx);
      }
      throw new Error(
        `Value '${display(tsValue)}' does not match any of the enum values: ${b.cases.join(', ')}`,
      );
    }

    case 'result':
      return serializeResult(tsValue, b, defs);

    case 'quota-token':
      if (!(tsValue instanceof QuotaToken)) throw typeMismatch(tsValue, 'QuotaToken');
      return tsValue._toSchemaValue(QUOTA_INTERNAL);

    case 'flags':
      throw new Error(`Serializing 'flags' values is not supported`);

    case 'ref':
      throw internalError(`unresolved ref '${b.id}'`);
  }
}

function serializeList(
  tsValue: any,
  element: ResolvedType,
  typedArray: TypedArrayKind | undefined,
  defs: Defs,
): SchemaValue {
  if (typedArray) {
    const spec = TYPED_ARRAYS[typedArray];
    if (!spec.is(tsValue)) throw typeMismatch(tsValue, spec.name);
    const arr = tsValue as ArrayLike<number | bigint>;
    const elems: SchemaValue[] = new Array(arr.length);
    for (let i = 0; i < arr.length; i++) elems[i] = serialize(arr[i], element, defs);
    return v.list(elems);
  }

  if (!Array.isArray(tsValue)) throw typeMismatch(tsValue, 'Array');
  return v.list(tsValue.map((item) => serialize(item, element, defs)));
}

function serializeRecord(tsValue: any, fields: ResolvedField[], defs: Defs): SchemaValue {
  if (typeof tsValue !== 'object' || tsValue === null) throw typeMismatch(tsValue, 'object');

  const values: SchemaValue[] = [];
  for (const f of fields) {
    if (!Object.prototype.hasOwnProperty.call(tsValue, f.name)) {
      if (f.type.body.tag === 'option') {
        values.push(v.option(undefined));
        continue;
      }
    }
    values.push(serialize(tsValue[f.name], f.type, defs));
  }
  return v.record(values);
}

function serializeVariant(
  tsValue: any,
  b: Extract<ResolvedType['body'], { tag: 'variant' }>,
  defs: Defs,
): SchemaValue {
  if (b.tagged) {
    if (typeof tsValue !== 'object' || tsValue === null) {
      throw typeMismatch(tsValue, 'object with tag property');
    }
    if (!('tag' in tsValue)) throw missingKey('tag', tsValue);

    for (let idx = 0; idx < b.cases.length; idx++) {
      const c = b.cases[idx];
      if (tsValue.tag !== c.name) continue;

      if (!c.payload) {
        return v.variant(idx);
      }
      if (!c.valueKey) {
        throw internalError(`Missing payload key for tagged case ${c.name}`);
      }
      if (!Object.prototype.hasOwnProperty.call(tsValue, c.valueKey)) {
        throw missingKey(c.valueKey, tsValue);
      }
      return v.variant(idx, serialize(tsValue[c.valueKey], c.payload, defs));
    }
    throw unionMismatch(b.cases, tsValue);
  }

  // Plain union
  for (let idx = 0; idx < b.cases.length; idx++) {
    const c = b.cases[idx];
    if (!c.payload) {
      if (tsValue === c.name) return v.variant(idx);
      continue;
    }
    if (matchesResolved(tsValue, c.payload, defs)) {
      return v.variant(idx, serialize(tsValue, c.payload, defs));
    }
  }
  throw unionMismatch(b.cases, tsValue);
}

function serializeResult(
  tsValue: any,
  b: Extract<ResolvedType['body'], { tag: 'result' }>,
  defs: Defs,
): SchemaValue {
  if (typeof tsValue !== 'object' || tsValue === null) throw typeMismatch(tsValue, 'object');
  if (!('tag' in tsValue)) throw missingKey('tag', tsValue);

  if (b.repr.tag === 'inbuilt') {
    // Parity with the legacy serializer: the inbuilt `Result` shape always
    // carries a `val` key (`Result.ok`/`Result.err` set it even for unit
    // payloads), so a missing `val` is a malformed value rather than a `none`.
    if (!Object.prototype.hasOwnProperty.call(tsValue, 'val')) {
      throw missingKey('val', tsValue);
    }
    if (tsValue.tag === 'ok') {
      if (b.ok) return v.ok(serialize(tsValue.val, b.ok, defs));
      if (b.repr.okAbsent !== undefined) return v.ok();
      throw internalError('unresolved ok type');
    }
    if (tsValue.tag === 'err') {
      if (b.err) return v.err(serialize(tsValue.val, b.err, defs));
      if (b.repr.errAbsent !== undefined) return v.err();
      throw internalError('unresolved err type');
    }
    throw typeMismatch(tsValue, 'Result');
  }

  // custom
  if (tsValue.tag === 'ok') {
    if (b.ok) {
      if (!b.repr.okValueName) throw internalError('unresolved key name for ok value');
      return v.ok(serialize(tsValue[b.repr.okValueName], b.ok, defs));
    }
    return v.ok();
  }
  if (tsValue.tag === 'err') {
    if (b.err) {
      if (!b.repr.errValueName) throw internalError('unresolved key name for err value');
      return v.err(serialize(tsValue[b.repr.errValueName], b.err, defs));
    }
    return v.err();
  }
  throw typeMismatch(tsValue, 'object with tag property');
}

// ============================================================
// Deserialization: SchemaValue -> TS value
// ============================================================

export function deserialize(value: SchemaValue, rt: ResolvedType, defs: Defs = EMPTY_DEFS): any {
  const b = resolveRef(rt, defs).body;
  switch (b.tag) {
    case 'bool':
      if (value.tag !== 'bool') throw deserializeMismatch(value, 'boolean');
      return value.value;

    case 'u8':
    case 'u16':
    case 'u32':
    case 's8':
    case 's16':
    case 's32':
    case 'f32':
    case 'f64':
      return asNumber(value);

    case 'u64':
    case 's64':
      return asBigInt(value);

    case 'char':
      if (value.tag !== 'char') throw deserializeMismatch(value, 'char');
      return value.value;
    case 'string':
      if (value.tag !== 'string') throw deserializeMismatch(value, 'string');
      return value.value;

    case 'option': {
      if (value.tag !== 'option') throw deserializeMismatch(value, 'option');
      if (value.value === undefined) return b.noneRepr === 'null' ? null : undefined;
      return deserialize(value.value, b.element, defs);
    }

    case 'list': {
      if (value.tag !== 'list') throw deserializeMismatch(value, 'list');
      if (b.typedArray) {
        const spec = TYPED_ARRAYS[b.typedArray];
        const arr = spec.make(value.elements.length) as any;
        for (let i = 0; i < value.elements.length; i++) {
          arr[i] = deserialize(value.elements[i], b.element, defs);
        }
        return arr;
      }
      return value.elements.map((e) => deserialize(e, b.element, defs));
    }

    case 'map': {
      if (value.tag !== 'map') throw deserializeMismatch(value, 'map');
      const map = new Map();
      for (const entry of value.entries) {
        map.set(deserialize(entry.key, b.key, defs), deserialize(entry.value, b.value, defs));
      }
      return map;
    }

    case 'tuple': {
      if (value.tag !== 'tuple') throw deserializeMismatch(value, 'tuple');
      if (b.empty !== undefined) {
        if (value.elements.length !== 0) throw deserializeMismatch(value, 'empty tuple');
        return b.empty === 'null' ? null : undefined;
      }
      if (value.elements.length !== b.elements.length) {
        throw deserializeMismatch(value, 'tuple');
      }
      return b.elements.map((et, i) => deserialize(value.elements[i], et, defs));
    }

    case 'record': {
      if (value.tag !== 'record') throw deserializeMismatch(value, 'record');
      const obj: Record<string, any> = {};
      for (let i = 0; i < b.fields.length; i++) {
        obj[b.fields[i].name] = deserialize(value.fields[i], b.fields[i].type, defs);
      }
      return obj;
    }

    case 'variant':
      return deserializeVariant(value, b, defs);

    case 'enum':
      if (value.tag !== 'enum') throw deserializeMismatch(value, 'enum');
      return b.cases[value.caseIndex];

    case 'result':
      return deserializeResult(value, b, defs);

    case 'quota-token':
      if (value.tag !== 'quota-token') throw deserializeMismatch(value, 'QuotaToken');
      return QuotaToken._fromSchemaValue(QUOTA_INTERNAL, value);

    case 'flags':
      throw new Error(`Deserializing 'flags' values is not supported`);

    case 'ref':
      throw internalError(`unresolved ref '${b.id}'`);
  }
}

function asNumber(value: SchemaValue): number {
  switch (value.tag) {
    case 'u8':
    case 'u16':
    case 'u32':
    case 's8':
    case 's16':
    case 's32':
    case 'f32':
    case 'f64':
      return value.value;
    case 'u64':
    case 's64':
      return Number(value.value);
    default:
      throw deserializeMismatch(value, 'number');
  }
}

function asBigInt(value: SchemaValue): bigint {
  if (value.tag === 'u64' || value.tag === 's64') return value.value;
  throw deserializeMismatch(value, 'bigint');
}

function deserializeVariant(
  value: SchemaValue,
  b: Extract<ResolvedType['body'], { tag: 'variant' }>,
  defs: Defs,
): any {
  if (value.tag !== 'variant') throw deserializeMismatch(value, 'variant');

  const c = b.cases[value.caseIndex];
  if (!c) throw deserializeMismatch(value, 'variant');

  if (b.tagged) {
    if (!c.payload) return { tag: c.name };
    if (value.payload === undefined) {
      if (c.payload.body.tag === 'option') return { tag: c.name };
      throw deserializeMismatch(value, 'variant');
    }
    if (!c.valueKey) throw deserializeMismatch(value, 'variant');
    return { tag: c.name, [c.valueKey]: deserialize(value.payload, c.payload, defs) };
  }

  // Plain union
  if (!c.payload) return c.name;
  if (value.payload === undefined) throw deserializeMismatch(value, 'variant');
  return deserialize(value.payload, c.payload, defs);
}

function deserializeResult(
  value: SchemaValue,
  b: Extract<ResolvedType['body'], { tag: 'result' }>,
  defs: Defs,
): any {
  if (value.tag !== 'result') throw deserializeMismatch(value, 'result');

  const res = value.result;

  if (b.repr.tag === 'inbuilt') {
    if (res.tag === 'ok') {
      if (b.ok && res.value !== undefined) return Result.ok(deserialize(res.value, b.ok, defs));
      if (res.value === undefined && b.repr.okAbsent !== undefined) {
        return Result.ok(b.repr.okAbsent === 'null' ? null : undefined);
      }
    }
    if (res.tag === 'err') {
      if (b.err && res.value !== undefined) return Result.err(deserialize(res.value, b.err, defs));
      if (res.value === undefined && b.repr.errAbsent !== undefined) {
        return Result.err(b.repr.errAbsent === 'null' ? null : undefined);
      }
    }
    throw deserializeMismatch(value, 'result');
  }

  // custom
  const okName = b.repr.okValueName;
  const errName = b.repr.errValueName;
  const okType = b.ok;
  const errType = b.err;

  if (okName && errName && okType && errType) {
    if (res.tag === 'ok' && res.value !== undefined) {
      return { tag: 'ok', [okName]: deserialize(res.value, okType, defs) };
    }
    if (res.tag === 'err' && res.value !== undefined) {
      return { tag: 'err', [errName]: deserialize(res.value, errType, defs) };
    }
  }

  if (okName && okType && !errType) {
    if (res.tag === 'ok' && res.value !== undefined) {
      return { tag: 'ok', [okName]: deserialize(res.value, okType, defs) };
    }
    return { tag: 'err' };
  }

  if (errName && errType && !okType) {
    if (res.tag === 'err' && res.value !== undefined) {
      return { tag: 'err', [errName]: deserialize(res.value, errType, defs) };
    }
    return { tag: 'ok' };
  }

  if (okName && !okType && res.tag === 'ok' && res.value === undefined) {
    return { tag: 'ok', [okName]: undefined };
  }
  if (errName && !errType && res.tag === 'err' && res.value === undefined) {
    return { tag: 'err', [errName]: undefined };
  }

  throw deserializeMismatch(value, 'result');
}

// ============================================================
// Structural matching (for plain-union case selection)
// ============================================================

export function matchesResolved(value: any, rt: ResolvedType, defs: Defs = EMPTY_DEFS): boolean {
  const b = resolveRef(rt, defs).body;
  const valueType = typeof value;

  switch (b.tag) {
    case 'bool':
      return valueType === 'boolean';
    case 'f64':
    case 'f32':
    case 's32':
    case 's16':
    case 's8':
    case 'u32':
    case 'u16':
    case 'u8':
      return valueType === 'number';
    case 's64':
      return valueType === 'bigint';
    case 'u64':
      return valueType === 'number' || valueType === 'bigint';
    case 'char':
    case 'string':
      return valueType === 'string';

    case 'option':
      return value === undefined || value === null || matchesResolved(value, b.element, defs);

    case 'list': {
      if (b.typedArray) return TYPED_ARRAYS[b.typedArray].is(value);
      if (!Array.isArray(value)) return false;
      return value.every((item) => matchesResolved(item, b.element, defs));
    }

    case 'map': {
      if (!(value instanceof Map)) return false;
      return Array.from(value.entries()).every(
        ([k, val]) => matchesResolved(k, b.key, defs) && matchesResolved(val, b.value, defs),
      );
    }

    case 'tuple': {
      if (b.empty !== undefined) return value === null || value === undefined;
      if (!Array.isArray(value)) return false;
      if (value.length !== b.elements.length) return false;
      return value.every((item, i) => matchesResolved(item, b.elements[i], defs));
    }

    case 'record':
      return matchesRecord(value, b.fields, defs);

    case 'enum':
      return valueType === 'string' && b.cases.includes(value);

    case 'variant':
      return matchesVariant(value, b, defs);

    case 'result': {
      if (valueType !== 'object' || value === null) return false;
      if ('ok' in value) {
        if (value.ok === undefined || value.ok === null) return b.ok === undefined;
        return b.ok ? matchesResolved(value.ok, b.ok, defs) : false;
      }
      if ('err' in value) {
        if (value.err === undefined || value.err === null) return b.err === undefined;
        return b.err ? matchesResolved(value.err, b.err, defs) : false;
      }
      return false;
    }

    case 'quota-token':
      return value instanceof QuotaToken;

    case 'flags':
      return false;

    case 'ref':
      throw internalError(`unresolved ref '${b.id}'`);
  }
}

function matchesRecord(value: any, fields: ResolvedField[], defs: Defs): boolean {
  if (typeof value !== 'object' || value === null) return false;
  if (Object.keys(value).length !== fields.length) return false;

  for (const f of fields) {
    const hasKey = Object.prototype.hasOwnProperty.call(value, f.name);
    const isOptional = f.type.body.tag === 'option';
    if (!hasKey) {
      if (!isOptional) return false;
    } else if (!matchesResolved(value[f.name], f.type, defs)) {
      return false;
    }
  }
  return true;
}

function matchesVariant(
  value: any,
  b: Extract<ResolvedType['body'], { tag: 'variant' }>,
  defs: Defs,
): boolean {
  if (value === null || value === undefined) return false;

  if (b.tagged && typeof value === 'object' && 'tag' in value) {
    const tagValue = value.tag;
    // Parity with the legacy matcher: only attempt tagged matching when the
    // `tag` is a string; otherwise fall through to plain structural matching.
    if (typeof tagValue === 'string') {
      const c = b.cases.find((x) => x.name === tagValue);
      if (!c) return false;
      if (!c.payload) return Object.keys(value).length === 1;
      if (!c.valueKey) return false;
      if (!Object.prototype.hasOwnProperty.call(value, c.valueKey)) return false;
      return matchesResolved(value[c.valueKey], c.payload, defs);
    }
  }

  for (const c of b.cases) {
    if (!c.payload) {
      if (typeof value === 'string' && value === c.name) return true;
      continue;
    }
    if (matchesResolved(value, c.payload, defs)) return true;
  }
  return false;
}
