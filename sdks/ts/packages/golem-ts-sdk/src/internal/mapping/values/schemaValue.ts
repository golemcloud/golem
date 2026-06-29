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
import {
  drainUnconsumedQuotaHandles,
  GuestSecretHandle,
  preflightWitValueTree,
  SchemaValue,
  v,
} from '../../schema-model';
import { SECRET_INTERNAL } from '../../schema-model/secretInternal';
import { QUOTA_INTERNAL } from '../../schema-model/quotaInternal';
import { GuestQuotaTokenHandle } from '../../schema-model/quotaTokenHandle';
import type {
  SchemaValueTree as WitSchemaValueTree,
  SchemaValueNode as WitSchemaValueNode,
  ValueNodeIndex,
} from 'golem:core/types@2.0.0';
import { SchemaDecodeError } from '../../schema-model';
import { Result } from '../../../host/result';
import { Secret } from '../../../agentConfig';
import { QuotaToken } from '../../../host/quota';
import { Duration, Path, Quantity } from '../../../richTypes';

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

type NarrowIntTag = 's8' | 's16' | 's32' | 'u8' | 'u16' | 'u32';
type WideIntTag = 's64' | 'u64';

const INT_RANGES: Record<NarrowIntTag, readonly [number, number]> = {
  s8: [-128, 127],
  s16: [-32768, 32767],
  s32: [-2147483648, 2147483647],
  u8: [0, 255],
  u16: [0, 65535],
  u32: [0, 4294967295],
};

const BIGINT_RANGES: Record<WideIntTag, readonly [bigint, bigint]> = {
  s64: [-(1n << 63n), (1n << 63n) - 1n],
  u64: [0n, (1n << 64n) - 1n],
};

function checkIntRange(tag: NarrowIntTag, value: number): void {
  const [min, max] = INT_RANGES[tag];
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${tag} value out of range: ${value}`);
  }
}

function checkBigIntRange(tag: WideIntTag, value: bigint): void {
  const [min, max] = BIGINT_RANGES[tag];
  if (value < min || value > max) {
    throw new Error(`${tag} value out of range: ${value}`);
  }
}

function checkCharValue(value: string): void {
  const codePoints = [...value];
  const cp = codePoints.length === 1 ? codePoints[0]!.codePointAt(0)! : undefined;
  if (cp === undefined || (cp >= 0xd800 && cp <= 0xdfff)) {
    throw new Error(`char value must be a single Unicode scalar value: ${JSON.stringify(value)}`);
  }
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

function dateToDatetime(value: Date): { seconds: bigint; nanoseconds: number } {
  const milliseconds = BigInt(value.getTime());
  let seconds = milliseconds / 1000n;
  let ms = milliseconds % 1000n;
  if (ms < 0n) {
    seconds -= 1n;
    ms += 1000n;
  }
  return {
    seconds,
    nanoseconds: Number(ms) * 1_000_000,
  };
}

function datetimeToDate(value: { seconds: bigint; nanoseconds: number }): Date {
  return new Date(
    Number(value.seconds * 1000n + BigInt(Math.trunc(value.nanoseconds / 1_000_000))),
  );
}

// ============================================================
// Ref resolution
// ============================================================

/** A registry of named composite definitions, keyed by stable `type-id`. */
export type Defs = ReadonlyMap<TypeId, ResolvedType>;

const EMPTY_DEFS: Defs = new Map();

// Body tags eligible for the specialized leaf list fast path (see `leafEncoder` /
// `leafDecoder`). Probed inline with a native `Set.has` before building the
// per-element closure so that lists of composite elements — e.g. the recursive
// `ref` children of a tree, of which there is one per node — skip the closure
// builder call entirely; an interpreted function call per list is a measurable
// cost under QuickJS on such workloads. This Set and the `leafEncoder` /
// `leafDecoder` switches must list the same tags; a mismatch only forgoes the
// fast path (the generic path is always a correct fallback), never correctness.
const LEAF_LIST_TAGS: ReadonlySet<string> = new Set([
  'bool',
  'u8',
  'u16',
  'u32',
  's8',
  's16',
  's32',
  'u64',
  's64',
  'f32',
  'f64',
  'char',
  'string',
  'enum',
]);

/** Follow `ref` bodies through `defs` until a concrete (non-ref) type is reached. */
function resolveRef(rt: ResolvedType, defs: Defs): ResolvedType {
  // Fast path: the overwhelming majority of types are not `ref`s (only recursive
  // back-edges are). Returning before touching the cycle guard avoids allocating
  // a `Set` per element when (de)serializing large lists/records, which is pure
  // waste for non-ref types and dominates the cost at scale.
  if (rt.body.tag !== 'ref') return rt;

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
  // The `v.*` builders are inlined as object literals on this hot path: in the
  // QuickJS interpreter every builder call is a real (non-inlined) frame, so for
  // large lists/records collapsing them into literals removes one call per node.
  switch (b.tag) {
    case 'bool':
      if (typeof tsValue !== 'boolean') throw typeMismatch(tsValue, 'boolean');
      return { tag: 'bool', value: tsValue };

    case 'f32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 'f32', value: tsValue };
    case 'f64':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 'f64', value: tsValue };
    case 'u8':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 'u8', value: tsValue };
    case 'u16':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 'u16', value: tsValue };
    case 'u32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 'u32', value: tsValue };
    case 's8':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 's8', value: tsValue };
    case 's16':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 's16', value: tsValue };
    case 's32':
      if (typeof tsValue !== 'number') throw typeMismatch(tsValue, 'number');
      return { tag: 's32', value: tsValue };

    case 'u64':
      if (typeof tsValue === 'bigint') {
        checkBigIntRange('u64', tsValue);
        return { tag: 'u64', value: tsValue };
      }
      if (typeof tsValue === 'number') {
        const value = BigInt(tsValue);
        checkBigIntRange('u64', value);
        return { tag: 'u64', value };
      }
      throw typeMismatch(tsValue, 'bigint');
    case 's64':
      if (typeof tsValue !== 'bigint') throw typeMismatch(tsValue, 'bigint');
      checkBigIntRange('s64', tsValue);
      return { tag: 's64', value: tsValue };

    case 'char':
      if (typeof tsValue !== 'string') throw typeMismatch(tsValue, 'string');
      return { tag: 'char', value: tsValue };
    case 'string':
      if (typeof tsValue !== 'string') throw typeMismatch(tsValue, 'string');
      return { tag: 'string', value: tsValue };

    case 'option':
      if (tsValue === null || tsValue === undefined) return { tag: 'option', value: undefined };
      return { tag: 'option', value: serialize(tsValue, b.element, defs) };

    case 'list':
      return serializeList(tsValue, b.element, b.typedArray, defs);

    case 'map': {
      if (!(tsValue instanceof Map)) throw typeMismatch(tsValue, 'Map');
      const entries = Array.from(tsValue.entries()).map(([k, val]) => ({
        key: serialize(k, b.key, defs),
        value: serialize(val, b.value, defs),
      }));
      return { tag: 'map', entries };
    }

    case 'tuple': {
      if (b.empty !== undefined) {
        if (tsValue === null || tsValue === undefined) return { tag: 'tuple', elements: [] };
        throw typeMismatch(tsValue, 'empty tuple');
      }
      if (!Array.isArray(tsValue) || tsValue.length !== b.elements.length) {
        throw typeMismatch(tsValue, `Array of length ${b.elements.length}`);
      }
      return { tag: 'tuple', elements: b.elements.map((et, i) => serialize(tsValue[i], et, defs)) };
    }

    case 'record':
      return serializeRecord(tsValue, b.fields, defs);

    case 'variant':
      return serializeVariant(tsValue, b, defs);

    case 'enum': {
      if (typeof tsValue === 'string') {
        const idx = b.cases.indexOf(tsValue);
        if (idx !== -1) return { tag: 'enum', caseIndex: idx };
      }
      throw new Error(
        `Value '${display(tsValue)}' does not match any of the enum values: ${b.cases.join(', ')}`,
      );
    }

    case 'result':
      return serializeResult(tsValue, b, defs);

    case 'secret':
      if (!(tsValue instanceof Secret)) throw typeMismatch(tsValue, 'Secret');
      return tsValue._toSchemaValue(SECRET_INTERNAL);

    case 'quota-token':
      if (!(tsValue instanceof QuotaToken)) throw typeMismatch(tsValue, 'QuotaToken');
      return tsValue._toSchemaValue(QUOTA_INTERNAL);

    case 'path':
      if (!(tsValue instanceof Path)) throw typeMismatch(tsValue, 'Path');
      return { tag: 'path', value: tsValue.path };
    case 'url':
      if (!(tsValue instanceof URL)) throw typeMismatch(tsValue, 'URL');
      return { tag: 'url', value: tsValue.toString() };
    case 'datetime':
      if (!(tsValue instanceof Date)) throw typeMismatch(tsValue, 'Date');
      return { tag: 'datetime', value: dateToDatetime(tsValue) };
    case 'duration':
      if (!(tsValue instanceof Duration)) throw typeMismatch(tsValue, 'Duration');
      return { tag: 'duration', nanoseconds: tsValue.nanoseconds };
    case 'quantity':
      if (!(tsValue instanceof Quantity)) throw typeMismatch(tsValue, 'Quantity');
      return { tag: 'quantity', value: tsValue.value };

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
    return { tag: 'list', elements: elems };
  }

  if (!Array.isArray(tsValue)) throw typeMismatch(tsValue, 'Array');
  return { tag: 'list', elements: tsValue.map((item) => serialize(item, element, defs)) };
}

function serializeRecord(tsValue: any, fields: ResolvedField[], defs: Defs): SchemaValue {
  if (typeof tsValue !== 'object' || tsValue === null) throw typeMismatch(tsValue, 'object');

  const values: SchemaValue[] = [];
  for (const f of fields) {
    if (!Object.prototype.hasOwnProperty.call(tsValue, f.name)) {
      if (f.type.body.tag === 'option') {
        values.push({ tag: 'option', value: undefined });
        continue;
      }
    }
    values.push(serialize(tsValue[f.name], f.type, defs));
  }
  return { tag: 'record', fields: values };
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
        return { tag: 'variant', caseIndex: idx, payload: undefined };
      }
      if (!c.valueKey) {
        throw internalError(`Missing payload key for tagged case ${c.name}`);
      }
      if (!Object.prototype.hasOwnProperty.call(tsValue, c.valueKey)) {
        throw missingKey(c.valueKey, tsValue);
      }
      return {
        tag: 'variant',
        caseIndex: idx,
        payload: serialize(tsValue[c.valueKey], c.payload, defs),
      };
    }
    throw unionMismatch(b.cases, tsValue);
  }

  // Plain union
  for (let idx = 0; idx < b.cases.length; idx++) {
    const c = b.cases[idx];
    if (!c.payload) {
      if (tsValue === c.name) return { tag: 'variant', caseIndex: idx, payload: undefined };
      continue;
    }
    if (matchesResolved(tsValue, c.payload, defs)) {
      return { tag: 'variant', caseIndex: idx, payload: serialize(tsValue, c.payload, defs) };
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
// Fused serialization: TS value -> flat wire `schema-value-tree`
// ============================================================

// `serializeGraphToWit` fuses `serialize` (TS value -> recursive `SchemaValue`)
// and `schemaValueToWit` (`SchemaValue` -> flat wire tree) into a single pass.
// On the hot invocation / RPC paths this removes the intermediate `SchemaValue`
// tree entirely (one full allocation + traversal per direction). The flattening
// is identical post-order to the two-step path — children are emitted before
// their parent and the root is the last node — so the produced `value-nodes`
// array and `root` index are byte-for-byte equal to
// `schemaValueToWit(serializeGraph(value, graph))`. That equality is relied on
// for stable agent ids (constructor inputs are hashed by `makeAgentId`).

/**
 * A reusable fused encoder over a single shared `value-nodes` pool. The boundary
 * layer creates one encoder and emits several runtime-parameter fields into the
 * same pool (each via {@link WireEncoder.emitGraph} with that field's own
 * `defs`), then closes the record with {@link WireEncoder.pushRecord}. The
 * standalone {@link serializeGraphToWit} is a thin single-root wrapper.
 */
export interface WireEncoder {
  readonly valueNodes: WitSchemaValueNode[];
  /** Emit a TS value against a `ResolvedGraph` root into the shared pool; returns its node index. */
  emitGraph(tsValue: any, graph: ResolvedGraph): ValueNodeIndex;
  /** Push a `record-value` whose fields are the given (already-emitted) node indices. */
  pushRecord(fieldIndices: ValueNodeIndex[]): ValueNodeIndex;
}

export function createWireEncoder(): WireEncoder {
  const valueNodes: WitSchemaValueNode[] = [];
  let defs: Defs = EMPTY_DEFS;
  // Per-`emitGraph` deferred quota-token takes. The raw owned resource is only
  // moved out of its take-once cell after the whole walk succeeds (see
  // `emitGraph`), so a sibling that fails mid-walk leaves the caller's token
  // intact — atomic, matching the non-fused `schemaValueToWit` preflight.
  // `seenRaw` rejects the same handle appearing twice in one tree before any
  // move. Both are reset for every `emitGraph` call (each call is one tree).
  let pendingTakes: { idx: number; handle: GuestSecretHandle | GuestQuotaTokenHandle }[] = [];
  let seenRaw: Set<unknown> = new Set();

  function push(node: WitSchemaValueNode): ValueNodeIndex {
    valueNodes.push(node);
    return valueNodes.length - 1;
  }

  // Specialized per-element encoder for a list whose (already `resolveRef`-ed)
  // element body is a leaf (primitive or enum). Built once per list, it replaces
  // the per-element `emit` call — and its per-element `resolveRef` + `switch`
  // dispatch — with a tight closure that pushes the wire node directly, the
  // dominant per-element cost on large lists. Returns `null` for composite
  // elements, which fall back to the generic `emit` path. The emitted wire tag
  // is identical to what `emit` produces for the same element type, keeping the
  // fused output byte-for-byte equal to the two-step path.
  function leafEncoder(eb: ResolvedType['body']): ((x: any) => ValueNodeIndex) | null {
    switch (eb.tag) {
      case 'bool':
        return (x) => {
          if (typeof x !== 'boolean') throw typeMismatch(x, 'boolean');
          return valueNodes.push({ tag: 'bool-value', val: x }) - 1;
        };
      case 'f32':
      case 'f64':
        return (x) => {
          if (typeof x !== 'number') throw typeMismatch(x, 'number');
          return valueNodes.push({ tag: `${eb.tag}-value`, val: x } as WitSchemaValueNode) - 1;
        };
      case 'u8':
      case 'u16':
      case 'u32':
      case 's8':
      case 's16':
      case 's32': {
        const wireTag = `${eb.tag}-value` as WitSchemaValueNode['tag'];
        return (x) => {
          if (typeof x !== 'number') throw typeMismatch(x, 'number');
          checkIntRange(eb.tag, x);
          return valueNodes.push({ tag: wireTag, val: x } as WitSchemaValueNode) - 1;
        };
      }
      case 'u64':
        return (x) => {
          if (typeof x === 'bigint') {
            checkBigIntRange('u64', x);
            return valueNodes.push({ tag: 'u64-value', val: x }) - 1;
          }
          if (typeof x === 'number') {
            const value = BigInt(x);
            checkBigIntRange('u64', value);
            return valueNodes.push({ tag: 'u64-value', val: value }) - 1;
          }
          throw typeMismatch(x, 'bigint');
        };
      case 's64':
        return (x) => {
          if (typeof x !== 'bigint') throw typeMismatch(x, 'bigint');
          checkBigIntRange('s64', x);
          return valueNodes.push({ tag: 's64-value', val: x }) - 1;
        };
      case 'char':
        return (x) => {
          if (typeof x !== 'string') throw typeMismatch(x, 'string');
          checkCharValue(x);
          return valueNodes.push({ tag: 'char-value', val: x }) - 1;
        };
      case 'string':
        return (x) => {
          if (typeof x !== 'string') throw typeMismatch(x, 'string');
          return valueNodes.push({ tag: 'string-value', val: x }) - 1;
        };
      case 'enum': {
        const cases = eb.cases;
        return (x) => {
          if (typeof x === 'string') {
            const idx = cases.indexOf(x);
            if (idx !== -1) return valueNodes.push({ tag: 'enum-value', val: idx }) - 1;
          }
          throw new Error(
            `Value '${display(x)}' does not match any of the enum values: ${cases.join(', ')}`,
          );
        };
      }
      default:
        return null;
    }
  }

  function emit(value: any, rt: ResolvedType): ValueNodeIndex {
    const b = resolveRef(rt, defs).body;
    switch (b.tag) {
      case 'bool':
        if (typeof value !== 'boolean') throw typeMismatch(value, 'boolean');
        return push({ tag: 'bool-value', val: value });

      case 'f32':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        return push({ tag: 'f32-value', val: value });
      case 'f64':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        return push({ tag: 'f64-value', val: value });
      case 'u8':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('u8', value);
        return push({ tag: 'u8-value', val: value });
      case 'u16':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('u16', value);
        return push({ tag: 'u16-value', val: value });
      case 'u32':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('u32', value);
        return push({ tag: 'u32-value', val: value });
      case 's8':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('s8', value);
        return push({ tag: 's8-value', val: value });
      case 's16':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('s16', value);
        return push({ tag: 's16-value', val: value });
      case 's32':
        if (typeof value !== 'number') throw typeMismatch(value, 'number');
        checkIntRange('s32', value);
        return push({ tag: 's32-value', val: value });

      case 'u64':
        if (typeof value === 'bigint') {
          checkBigIntRange('u64', value);
          return push({ tag: 'u64-value', val: value });
        }
        if (typeof value === 'number') {
          const bigintValue = BigInt(value);
          checkBigIntRange('u64', bigintValue);
          return push({ tag: 'u64-value', val: bigintValue });
        }
        throw typeMismatch(value, 'bigint');
      case 's64':
        if (typeof value !== 'bigint') throw typeMismatch(value, 'bigint');
        checkBigIntRange('s64', value);
        return push({ tag: 's64-value', val: value });

      case 'char':
        if (typeof value !== 'string') throw typeMismatch(value, 'string');
        checkCharValue(value);
        return push({ tag: 'char-value', val: value });
      case 'string':
        if (typeof value !== 'string') throw typeMismatch(value, 'string');
        return push({ tag: 'string-value', val: value });

      case 'option': {
        if (value === null || value === undefined)
          return push({ tag: 'option-value', val: undefined });
        const inner = emit(value, b.element);
        return push({ tag: 'option-value', val: inner });
      }

      case 'list': {
        // Probe `b.element.body` directly rather than via `resolveRef`: a `ref`
        // element (recursive type) is not a leaf, and resolving it here would pay
        // the `resolveRef` slow path (set allocation + chain walk) once per list
        // — pure waste for the many small `ref` lists in a recursive tree. Direct
        // primitive elements need no resolution, so this misses nothing. The
        // `Set.has` gate avoids even the `leafEncoder` call for composite lists.
        const eb = b.element.body;
        const leaf = LEAF_LIST_TAGS.has(eb.tag) ? leafEncoder(eb) : null;
        if (b.typedArray) {
          const spec = TYPED_ARRAYS[b.typedArray];
          if (!spec.is(value)) throw typeMismatch(value, spec.name);
          const arr = value as ArrayLike<number | bigint>;
          const idxs: ValueNodeIndex[] = new Array(arr.length);
          if (leaf) {
            for (let i = 0; i < arr.length; i++) idxs[i] = leaf(arr[i]);
          } else {
            for (let i = 0; i < arr.length; i++) idxs[i] = emit(arr[i], b.element);
          }
          return push({ tag: 'list-value', val: idxs });
        }
        if (!Array.isArray(value)) throw typeMismatch(value, 'Array');
        // Keep `Array.prototype.map`: under QuickJS the builtin iteration beats a
        // hand-rolled `for`+`push` loop; only the callback body is specialized.
        const idxs = leaf ? value.map(leaf) : value.map((item) => emit(item, b.element));
        return push({ tag: 'list-value', val: idxs });
      }

      case 'map': {
        if (!(value instanceof Map)) throw typeMismatch(value, 'Map');
        const entries = Array.from(value.entries()).map(([k, val]) => ({
          key: emit(k, b.key),
          value: emit(val, b.value),
        }));
        return push({ tag: 'map-value', val: entries });
      }

      case 'tuple': {
        if (b.empty !== undefined) {
          if (value === null || value === undefined) return push({ tag: 'tuple-value', val: [] });
          throw typeMismatch(value, 'empty tuple');
        }
        if (!Array.isArray(value) || value.length !== b.elements.length) {
          throw typeMismatch(value, `Array of length ${b.elements.length}`);
        }
        return push({
          tag: 'tuple-value',
          val: b.elements.map((et, i) => emit(value[i], et)),
        });
      }

      case 'record':
        return emitRecord(value, b.fields);

      case 'variant':
        return emitVariant(value, b);

      case 'enum': {
        if (typeof value === 'string') {
          const idx = b.cases.indexOf(value);
          if (idx !== -1) return push({ tag: 'enum-value', val: idx });
        }
        throw new Error(
          `Value '${display(value)}' does not match any of the enum values: ${b.cases.join(', ')}`,
        );
      }

      case 'result':
        return emitResult(value, b);

      case 'secret': {
        if (!(value instanceof Secret)) throw typeMismatch(value, 'Secret');
        const secretValue = value._toSchemaValue(SECRET_INTERNAL);
        if (secretValue.tag !== 'secret') throw internalError('expected secret value');
        const handle = secretValue.handle;
        const raw = handle.withHandle((r) => r);
        if (raw === undefined) {
          throw new Error(
            'secret handle was already transferred; an owned secret can only be sent once',
          );
        }
        if (seenRaw.has(raw)) {
          throw new Error('the same secret handle appeared more than once in one value tree');
        }
        seenRaw.add(raw);
        const idx = push({ tag: 'secret-value', val: undefined } as unknown as WitSchemaValueNode);
        pendingTakes.push({ idx, handle });
        return idx;
      }

      case 'quota-token': {
        if (!(value instanceof QuotaToken)) throw typeMismatch(value, 'QuotaToken');
        const quotaValue = value._toSchemaValue(QUOTA_INTERNAL);
        if (quotaValue.tag !== 'quota-token') throw internalError('expected quota-token value');
        const handle = quotaValue.handle;
        // Peek the underlying owned resource without consuming it. If this
        // throws (already transferred, or aliased), nothing has been moved out,
        // so the caller still owns its token. The take is deferred until
        // `emitGraph` commits after a successful walk.
        const raw = handle.withHandle((r) => r);
        if (raw === undefined) {
          throw new Error(
            'quota-token handle was already transferred; an owned quota-token can only be sent once',
          );
        }
        if (seenRaw.has(raw)) {
          throw new Error('the same quota-token handle appeared more than once in one value tree');
        }
        seenRaw.add(raw);
        const idx = push({
          tag: 'quota-token-handle',
          val: undefined,
        } as unknown as WitSchemaValueNode);
        pendingTakes.push({ idx, handle });
        return idx;
      }

      case 'path':
        if (!(value instanceof Path)) throw typeMismatch(value, 'Path');
        return push({ tag: 'path-value', val: value.path });
      case 'url':
        if (!(value instanceof URL)) throw typeMismatch(value, 'URL');
        return push({ tag: 'url-value', val: value.toString() });
      case 'datetime':
        if (!(value instanceof Date)) throw typeMismatch(value, 'Date');
        return push({ tag: 'datetime-value', val: dateToDatetime(value) });
      case 'duration':
        if (!(value instanceof Duration)) throw typeMismatch(value, 'Duration');
        return push({ tag: 'duration-value', val: { nanoseconds: value.nanoseconds } });
      case 'quantity':
        if (!(value instanceof Quantity)) throw typeMismatch(value, 'Quantity');
        return push({ tag: 'quantity-value-node', val: value.value });

      case 'flags':
        throw new Error(`Serializing 'flags' values is not supported`);

      case 'ref':
        throw internalError(`unresolved ref '${b.id}'`);
    }
  }

  function emitRecord(value: any, fields: ResolvedField[]): ValueNodeIndex {
    if (typeof value !== 'object' || value === null) throw typeMismatch(value, 'object');
    const idxs: ValueNodeIndex[] = [];
    for (const f of fields) {
      if (!Object.prototype.hasOwnProperty.call(value, f.name) && f.type.body.tag === 'option') {
        idxs.push(push({ tag: 'option-value', val: undefined }));
        continue;
      }
      idxs.push(emit(value[f.name], f.type));
    }
    return push({ tag: 'record-value', val: idxs });
  }

  function emitVariant(
    value: any,
    b: Extract<ResolvedType['body'], { tag: 'variant' }>,
  ): ValueNodeIndex {
    if (b.tagged) {
      if (typeof value !== 'object' || value === null) {
        throw typeMismatch(value, 'object with tag property');
      }
      if (!('tag' in value)) throw missingKey('tag', value);

      for (let idx = 0; idx < b.cases.length; idx++) {
        const c = b.cases[idx];
        if (value.tag !== c.name) continue;

        if (!c.payload) {
          return push({ tag: 'variant-value', val: { case_: idx, payload: undefined } });
        }
        if (!c.valueKey) {
          throw internalError(`Missing payload key for tagged case ${c.name}`);
        }
        if (!Object.prototype.hasOwnProperty.call(value, c.valueKey)) {
          throw missingKey(c.valueKey, value);
        }
        const payload = emit(value[c.valueKey], c.payload);
        return push({ tag: 'variant-value', val: { case_: idx, payload } });
      }
      throw unionMismatch(b.cases, value);
    }

    // Plain union
    for (let idx = 0; idx < b.cases.length; idx++) {
      const c = b.cases[idx];
      if (!c.payload) {
        if (value === c.name)
          return push({ tag: 'variant-value', val: { case_: idx, payload: undefined } });
        continue;
      }
      if (matchesResolved(value, c.payload, defs)) {
        const payload = emit(value, c.payload);
        return push({ tag: 'variant-value', val: { case_: idx, payload } });
      }
    }
    throw unionMismatch(b.cases, value);
  }

  function emitResult(
    value: any,
    b: Extract<ResolvedType['body'], { tag: 'result' }>,
  ): ValueNodeIndex {
    if (typeof value !== 'object' || value === null) throw typeMismatch(value, 'object');
    if (!('tag' in value)) throw missingKey('tag', value);

    if (b.repr.tag === 'inbuilt') {
      if (!Object.prototype.hasOwnProperty.call(value, 'val')) {
        throw missingKey('val', value);
      }
      if (value.tag === 'ok') {
        if (b.ok) {
          const inner = emit(value.val, b.ok);
          return push({ tag: 'result-value', val: { tag: 'ok-value', val: inner } });
        }
        if (b.repr.okAbsent !== undefined)
          return push({ tag: 'result-value', val: { tag: 'ok-value', val: undefined } });
        throw internalError('unresolved ok type');
      }
      if (value.tag === 'err') {
        if (b.err) {
          const inner = emit(value.val, b.err);
          return push({ tag: 'result-value', val: { tag: 'err-value', val: inner } });
        }
        if (b.repr.errAbsent !== undefined)
          return push({ tag: 'result-value', val: { tag: 'err-value', val: undefined } });
        throw internalError('unresolved err type');
      }
      throw typeMismatch(value, 'Result');
    }

    // custom
    if (value.tag === 'ok') {
      if (b.ok) {
        if (!b.repr.okValueName) throw internalError('unresolved key name for ok value');
        const inner = emit(value[b.repr.okValueName], b.ok);
        return push({ tag: 'result-value', val: { tag: 'ok-value', val: inner } });
      }
      return push({ tag: 'result-value', val: { tag: 'ok-value', val: undefined } });
    }
    if (value.tag === 'err') {
      if (b.err) {
        if (!b.repr.errValueName) throw internalError('unresolved key name for err value');
        const inner = emit(value[b.repr.errValueName], b.err);
        return push({ tag: 'result-value', val: { tag: 'err-value', val: inner } });
      }
      return push({ tag: 'result-value', val: { tag: 'err-value', val: undefined } });
    }
    throw typeMismatch(value, 'object with tag property');
  }

  return {
    valueNodes,
    emitGraph(tsValue: any, graph: ResolvedGraph): ValueNodeIndex {
      defs = graph.defs;
      pendingTakes = [];
      seenRaw = new Set();
      const root = emit(tsValue, graph.root);
      // The walk succeeded: commit every deferred take. `take()` cannot return
      // `undefined` here — the peek confirmed presence and uniqueness, and
      // nothing else moves the handle on this single thread — so the affine
      // move runs exactly once per handle, only on success.
      for (const { idx, handle } of pendingTakes) {
        const raw = handle.take();
        if (raw === undefined) {
          throw new Error(
            'owned handle was already transferred; an owned resource can only be sent once',
          );
        }
        (valueNodes[idx] as { val: unknown }).val = raw;
      }
      return root;
    },
    pushRecord(fieldIndices: ValueNodeIndex[]): ValueNodeIndex {
      return push({ tag: 'record-value', val: fieldIndices });
    },
  };
}

/** Fused encode: TS value -> flat wire `schema-value-tree`, guided by a `ResolvedGraph`. */
export function serializeGraphToWit(tsValue: any, graph: ResolvedGraph): WitSchemaValueTree {
  const enc = createWireEncoder();
  const root = enc.emitGraph(tsValue, graph);
  return { valueNodes: enc.valueNodes, root };
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
      if (value.fields.length !== b.fields.length) throw deserializeMismatch(value, 'record');
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
      if (
        !Number.isInteger(value.caseIndex) ||
        value.caseIndex < 0 ||
        value.caseIndex >= b.cases.length
      ) {
        throw deserializeMismatch(value, 'enum');
      }
      return b.cases[value.caseIndex];

    case 'result':
      return deserializeResult(value, b, defs);

    case 'secret':
      if (value.tag !== 'secret') throw deserializeMismatch(value, 'Secret');
      if (!(value.handle instanceof GuestSecretHandle)) throw deserializeMismatch(value, 'Secret');
      return Secret._fromSchemaValue(SECRET_INTERNAL, value, {
        defs: new Map(defs),
        root: b.inner,
      });

    case 'quota-token':
      if (value.tag !== 'quota-token') throw deserializeMismatch(value, 'QuotaToken');
      if (!(value.handle instanceof GuestQuotaTokenHandle)) {
        throw deserializeMismatch(value, 'QuotaToken');
      }
      return QuotaToken._fromSchemaValue(QUOTA_INTERNAL, value);

    case 'path':
      if (value.tag !== 'path') throw deserializeMismatch(value, 'Path');
      return new Path(value.value);
    case 'url':
      if (value.tag !== 'url') throw deserializeMismatch(value, 'URL');
      return new URL(value.value);
    case 'datetime':
      if (value.tag !== 'datetime') throw deserializeMismatch(value, 'Date');
      return datetimeToDate(value.value);
    case 'duration':
      if (value.tag !== 'duration') throw deserializeMismatch(value, 'Duration');
      return new Duration(value.nanoseconds);
    case 'quantity':
      if (value.tag !== 'quantity') throw deserializeMismatch(value, 'Quantity');
      return new Quantity(value.value);

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
    if (!c.payload) {
      if (value.payload !== undefined) throw deserializeMismatch(value, 'variant');
      return { tag: c.name };
    }
    if (value.payload === undefined) {
      throw deserializeMismatch(value, 'variant');
    }
    if (!c.valueKey) throw deserializeMismatch(value, 'variant');
    return { tag: c.name, [c.valueKey]: deserialize(value.payload, c.payload, defs) };
  }

  // Plain union
  if (!c.payload) {
    if (value.payload !== undefined) throw deserializeMismatch(value, 'variant');
    return c.name;
  }
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
    if (res.tag === 'err' && res.value === undefined) return { tag: 'err' };
    throw deserializeMismatch(value, 'result');
  }

  if (errName && errType && !okType) {
    if (res.tag === 'err' && res.value !== undefined) {
      return { tag: 'err', [errName]: deserialize(res.value, errType, defs) };
    }
    if (res.tag === 'ok' && res.value === undefined) return { tag: 'ok' };
    throw deserializeMismatch(value, 'result');
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
// Fused deserialization: flat wire `schema-value-tree` -> TS value
// ============================================================

// `deserializeGraphFromWit` fuses `schemaValueFromWit` (flat wire ->
// `SchemaValue`) and `deserialize` (`SchemaValue` -> TS value) into one pass,
// removing the intermediate `SchemaValue` tree on the hot decode path. The wire
// is untrusted, so this keeps the full validation of `schemaValueFromWit` — node
// index bounds checks plus the `onPath` cycle guard — while dispatching on the
// expected `ResolvedType` exactly like `deserialize`, validating every visited
// node's actual wire tag before reading its payload.

function wireMismatch(node: WitSchemaValueNode, expected: string): Error {
  return new SchemaDecodeError(
    `Failed to deserialize schema value node with tag \`${node.tag}\` to TypeScript type \`${expected}\``,
  );
}

function wireAsNumber(node: WitSchemaValueNode): number {
  switch (node.tag) {
    case 'u8-value':
    case 'u16-value':
    case 'u32-value':
    case 's8-value':
    case 's16-value':
    case 's32-value':
    case 'f32-value':
    case 'f64-value':
      return node.val;
    case 'u64-value':
    case 's64-value':
      return Number(node.val);
    default:
      throw wireMismatch(node, 'number');
  }
}

function wireAsBigInt(node: WitSchemaValueNode): bigint {
  if (node.tag === 'u64-value' || node.tag === 's64-value') return node.val;
  throw wireMismatch(node, 'bigint');
}

/**
 * A reusable fused decoder over a single shared wire `value-nodes` array and one
 * shared `onPath` cycle guard. The boundary layer reads an input record's field
 * indices via {@link WireDecoder.recordFieldIndices} and then decodes each field
 * with its own `defs` via {@link WireDecoder.readGraph}. The standalone
 * {@link deserializeGraphFromWit} is a thin single-root wrapper.
 */
export interface WireDecoder {
  /** Validate that the node at `idx` is a `record-value` and return its field node indices. */
  recordFieldIndices(idx: ValueNodeIndex): ValueNodeIndex[];
  /** Decode the node at `idx` against a `ResolvedGraph` root. */
  readGraph(idx: ValueNodeIndex, graph: ResolvedGraph): any;
}

export function createWireDecoder(nodes: WitSchemaValueNode[]): WireDecoder {
  let defs: Defs = EMPTY_DEFS;
  // See `schemaValueFromWit`: `1` = node currently on the DFS path. A back-edge
  // to an on-path node is a cycle; DAG sharing across sibling branches is fine.
  const onPath = new Uint8Array(nodes.length);
  // Per-`readGraph` record of quota-token lifts. When a `quota-token-handle`
  // node is lifted its raw resource is moved out of the wire node into a
  // `QuotaToken`; if a *later* sibling then fails the whole decode throws, and
  // these entries restore the wire node so the operation is atomic — the input
  // wire is left exactly as it was, for the runtime to release. `seenRaw`
  // rejects the same owned resource appearing in two distinct nodes (an affine
  // alias) before the second is lifted. Both reset per `readGraph` call.
  let pendingLifts: { node: WitSchemaValueNode; raw: unknown }[] = [];
  let seenRaw: Set<unknown> = new Set();

  function nodeAt(idx: ValueNodeIndex): WitSchemaValueNode {
    if (idx < 0 || idx >= nodes.length) {
      throw new SchemaDecodeError(`value node index out of range: ${idx} (nodes: ${nodes.length})`);
    }
    return nodes[idx];
  }

  function fromIdx(idx: ValueNodeIndex, rt: ResolvedType): any {
    if (idx < 0 || idx >= nodes.length) {
      throw new SchemaDecodeError(`value node index out of range: ${idx} (nodes: ${nodes.length})`);
    }
    if (onPath[idx] === 1) {
      throw new SchemaDecodeError(`cyclic value node reference at index ${idx}`);
    }
    onPath[idx] = 1;
    try {
      return fromNode(nodes[idx], rt);
    } finally {
      onPath[idx] = 0;
    }
  }

  // Specialized per-element decoder for a list whose (already `resolveRef`-ed)
  // element body is a leaf (primitive or enum). Built once per list, it replaces
  // the per-element `fromIdx` call — and its per-element `onPath` set/clear,
  // `resolveRef` and `switch` dispatch — with a tight closure. Skipping the
  // `onPath` cycle guard is safe for leaves: a primitive node carries no child
  // indices, so it can never close a cycle (the generic path sets `onPath` and
  // clears it again without recursing, so the guard never fires for a leaf).
  // Numeric decode laxness is preserved via the shared `wireAsNumber` /
  // `wireAsBigInt`. Returns `null` for composite elements, which fall back to
  // the generic `fromIdx` path.
  function leafDecoder(eb: ResolvedType['body']): ((idx: ValueNodeIndex) => any) | null {
    switch (eb.tag) {
      case 'bool':
        return (idx) => {
          const n = nodeAt(idx);
          if (n.tag !== 'bool-value') throw wireMismatch(n, 'boolean');
          return n.val;
        };
      case 'u8':
      case 'u16':
      case 'u32':
      case 's8':
      case 's16':
      case 's32':
      case 'f32':
      case 'f64':
        // `nodeAt` + `wireAsNumber` inlined: this is the dominant per-element
        // cost on the `list<number>` decode benchmark leg, and removing the two
        // inner calls is a measurable QuickJS win. Laxness matches `wireAsNumber`.
        return (idx) => {
          if (idx < 0 || idx >= nodes.length) {
            throw new SchemaDecodeError(
              `value node index out of range: ${idx} (nodes: ${nodes.length})`,
            );
          }
          const node = nodes[idx];
          switch (node.tag) {
            case 'u8-value':
            case 'u16-value':
            case 'u32-value':
            case 's8-value':
            case 's16-value':
            case 's32-value':
            case 'f32-value':
            case 'f64-value':
              return node.val;
            case 'u64-value':
            case 's64-value':
              return Number(node.val);
            default:
              throw wireMismatch(node, 'number');
          }
        };
      case 'u64':
      case 's64':
        return (idx) => {
          if (idx < 0 || idx >= nodes.length) {
            throw new SchemaDecodeError(
              `value node index out of range: ${idx} (nodes: ${nodes.length})`,
            );
          }
          const node = nodes[idx];
          if (node.tag === 'u64-value' || node.tag === 's64-value') return node.val;
          throw wireMismatch(node, 'bigint');
        };
      case 'char':
        return (idx) => {
          const n = nodeAt(idx);
          if (n.tag !== 'char-value') throw wireMismatch(n, 'char');
          return n.val;
        };
      case 'string':
        return (idx) => {
          const n = nodeAt(idx);
          if (n.tag !== 'string-value') throw wireMismatch(n, 'string');
          return n.val;
        };
      case 'enum': {
        const cases = eb.cases;
        return (idx) => {
          const n = nodeAt(idx);
          if (n.tag !== 'enum-value') throw wireMismatch(n, 'enum');
          if (!Number.isInteger(n.val) || n.val < 0 || n.val >= cases.length) {
            throw wireMismatch(n, 'enum');
          }
          return cases[n.val];
        };
      }
      default:
        return null;
    }
  }

  function fromNode(n: WitSchemaValueNode, rt: ResolvedType): any {
    const b = resolveRef(rt, defs).body;
    switch (b.tag) {
      case 'bool':
        if (n.tag !== 'bool-value') throw wireMismatch(n, 'boolean');
        return n.val;

      case 'u8':
      case 'u16':
      case 'u32':
      case 's8':
      case 's16':
      case 's32':
      case 'f32':
      case 'f64':
        return wireAsNumber(n);

      case 'u64':
      case 's64':
        return wireAsBigInt(n);

      case 'char':
        if (n.tag !== 'char-value') throw wireMismatch(n, 'char');
        return n.val;
      case 'string':
        if (n.tag !== 'string-value') throw wireMismatch(n, 'string');
        return n.val;

      case 'option': {
        if (n.tag !== 'option-value') throw wireMismatch(n, 'option');
        if (n.val === undefined) return b.noneRepr === 'null' ? null : undefined;
        return fromIdx(n.val, b.element);
      }

      case 'list': {
        if (n.tag !== 'list-value') throw wireMismatch(n, 'list');
        // Probe `b.element.body` directly rather than via `resolveRef`: see the
        // matching note in the encoder's `list` case. A `ref` element is not a
        // leaf and falls back to the generic per-element `fromIdx` path (which
        // resolves the ref and keeps the cycle guard). The `Set.has` gate avoids
        // even the `leafDecoder` call for composite lists.
        const eb = b.element.body;
        const leaf = LEAF_LIST_TAGS.has(eb.tag) ? leafDecoder(eb) : null;
        if (b.typedArray) {
          const spec = TYPED_ARRAYS[b.typedArray];
          const arr = spec.make(n.val.length) as any;
          if (leaf) {
            for (let i = 0; i < n.val.length; i++) arr[i] = leaf(n.val[i]);
          } else {
            for (let i = 0; i < n.val.length; i++) arr[i] = fromIdx(n.val[i], b.element);
          }
          return arr;
        }
        // Keep `Array.prototype.map`: under QuickJS the builtin iteration beats a
        // hand-rolled `for` loop; only the callback body is specialized.
        return leaf ? n.val.map(leaf) : n.val.map((i) => fromIdx(i, b.element));
      }

      case 'map': {
        if (n.tag !== 'map-value') throw wireMismatch(n, 'map');
        const map = new Map();
        for (const entry of n.val) {
          map.set(fromIdx(entry.key, b.key), fromIdx(entry.value, b.value));
        }
        return map;
      }

      case 'tuple': {
        if (n.tag !== 'tuple-value') throw wireMismatch(n, 'tuple');
        if (b.empty !== undefined) {
          if (n.val.length !== 0) throw wireMismatch(n, 'empty tuple');
          return b.empty === 'null' ? null : undefined;
        }
        if (n.val.length !== b.elements.length) throw wireMismatch(n, 'tuple');
        return b.elements.map((et, i) => fromIdx(n.val[i], et));
      }

      case 'record': {
        if (n.tag !== 'record-value') throw wireMismatch(n, 'record');
        if (n.val.length !== b.fields.length) throw wireMismatch(n, 'record');
        const obj: Record<string, any> = {};
        for (let i = 0; i < b.fields.length; i++) {
          obj[b.fields[i].name] = fromIdx(n.val[i], b.fields[i].type);
        }
        return obj;
      }

      case 'variant':
        return fromVariant(n, b);

      case 'enum':
        if (n.tag !== 'enum-value') throw wireMismatch(n, 'enum');
        if (!Number.isInteger(n.val) || n.val < 0 || n.val >= b.cases.length) {
          throw wireMismatch(n, 'enum');
        }
        return b.cases[n.val];

      case 'result':
        return fromResult(n, b);

      case 'secret': {
        if (n.tag !== 'secret-value') throw wireMismatch(n, 'Secret');
        const raw = n.val as typeof n.val | undefined;
        if (raw === undefined) {
          throw new SchemaDecodeError('secret handle referenced more than once');
        }
        if (seenRaw.has(raw)) {
          throw new SchemaDecodeError('the same secret resource appeared more than once');
        }
        seenRaw.add(raw);
        (n as { val: unknown }).val = undefined;
        pendingLifts.push({ node: n, raw });
        return Secret._fromHandle(
          SECRET_INTERNAL,
          GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw),
          { defs: new Map(defs), root: b.inner },
        );
      }

      case 'quota-token': {
        if (n.tag !== 'quota-token-handle') throw wireMismatch(n, 'QuotaToken');
        const raw = n.val as typeof n.val | undefined;
        if (raw === undefined) {
          throw new SchemaDecodeError('quota-token handle referenced more than once');
        }
        if (seenRaw.has(raw)) {
          throw new SchemaDecodeError('the same quota-token resource appeared more than once');
        }
        seenRaw.add(raw);
        // Lift the owned resource out of the wire node into a `QuotaToken`, but
        // record the lift so a later-sibling failure in `readGraph` can restore
        // the wire node — leaving the input wire untouched on a failed decode.
        (n as { val: unknown }).val = undefined;
        pendingLifts.push({ node: n, raw });
        return QuotaToken._fromHandle(
          QUOTA_INTERNAL,
          GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, raw),
        );
      }

      case 'path':
        if (n.tag !== 'path-value') throw wireMismatch(n, 'Path');
        return new Path(n.val);
      case 'url':
        if (n.tag !== 'url-value') throw wireMismatch(n, 'URL');
        return new URL(n.val);
      case 'datetime':
        if (n.tag !== 'datetime-value') throw wireMismatch(n, 'Date');
        return datetimeToDate(n.val);
      case 'duration':
        if (n.tag !== 'duration-value') throw wireMismatch(n, 'Duration');
        return new Duration(n.val.nanoseconds);
      case 'quantity':
        if (n.tag !== 'quantity-value-node') throw wireMismatch(n, 'Quantity');
        return new Quantity(n.val);

      case 'flags':
        throw new Error(`Deserializing 'flags' values is not supported`);

      case 'ref':
        throw internalError(`unresolved ref '${b.id}'`);
    }
  }

  function fromVariant(
    n: WitSchemaValueNode,
    b: Extract<ResolvedType['body'], { tag: 'variant' }>,
  ): any {
    if (n.tag !== 'variant-value') throw wireMismatch(n, 'variant');
    const nv = n.val;
    const c = b.cases[nv.case_];
    if (!c) throw wireMismatch(n, 'variant');

    if (b.tagged) {
      if (!c.payload) {
        if (nv.payload !== undefined) throw wireMismatch(n, 'variant');
        return { tag: c.name };
      }
      if (nv.payload === undefined) {
        throw wireMismatch(n, 'variant');
      }
      if (!c.valueKey) throw wireMismatch(n, 'variant');
      return { tag: c.name, [c.valueKey]: fromIdx(nv.payload, c.payload) };
    }

    // Plain union
    if (!c.payload) {
      if (nv.payload !== undefined) throw wireMismatch(n, 'variant');
      return c.name;
    }
    if (nv.payload === undefined) throw wireMismatch(n, 'variant');
    return fromIdx(nv.payload, c.payload);
  }

  function fromResult(
    n: WitSchemaValueNode,
    b: Extract<ResolvedType['body'], { tag: 'result' }>,
  ): any {
    if (n.tag !== 'result-value') throw wireMismatch(n, 'result');
    const r = n.val;
    if (r.tag !== 'ok-value' && r.tag !== 'err-value') {
      throw new SchemaDecodeError(
        `unknown result value payload tag '${(r as { tag: string }).tag}'`,
      );
    }
    const isOk = r.tag === 'ok-value';
    const hasVal = r.val !== undefined;

    if (b.repr.tag === 'inbuilt') {
      if (isOk) {
        if (b.ok && hasVal) return Result.ok(fromIdx(r.val!, b.ok));
        if (!hasVal && b.repr.okAbsent !== undefined) {
          return Result.ok(b.repr.okAbsent === 'null' ? null : undefined);
        }
      } else {
        if (b.err && hasVal) return Result.err(fromIdx(r.val!, b.err));
        if (!hasVal && b.repr.errAbsent !== undefined) {
          return Result.err(b.repr.errAbsent === 'null' ? null : undefined);
        }
      }
      throw wireMismatch(n, 'result');
    }

    // custom
    const okName = b.repr.okValueName;
    const errName = b.repr.errValueName;
    const okType = b.ok;
    const errType = b.err;

    if (okName && errName && okType && errType) {
      if (isOk && hasVal) return { tag: 'ok', [okName]: fromIdx(r.val!, okType) };
      if (!isOk && hasVal) return { tag: 'err', [errName]: fromIdx(r.val!, errType) };
    }

    if (okName && okType && !errType) {
      if (isOk && hasVal) return { tag: 'ok', [okName]: fromIdx(r.val!, okType) };
      if (!isOk && !hasVal) return { tag: 'err' };
      throw wireMismatch(n, 'result');
    }

    if (errName && errType && !okType) {
      if (!isOk && hasVal) return { tag: 'err', [errName]: fromIdx(r.val!, errType) };
      if (isOk && !hasVal) return { tag: 'ok' };
      throw wireMismatch(n, 'result');
    }

    if (okName && !okType && isOk && !hasVal) {
      return { tag: 'ok', [okName]: undefined };
    }
    if (errName && !errType && !isOk && !hasVal) {
      return { tag: 'err', [errName]: undefined };
    }

    throw wireMismatch(n, 'result');
  }

  return {
    recordFieldIndices(idx: ValueNodeIndex): ValueNodeIndex[] {
      if (idx < 0 || idx >= nodes.length) {
        throw new SchemaDecodeError(
          `value node index out of range: ${idx} (nodes: ${nodes.length})`,
        );
      }
      const node = nodes[idx];
      if (node.tag !== 'record-value') throw wireMismatch(node, 'record');
      return node.val;
    },
    readGraph(idx: ValueNodeIndex, graph: ResolvedGraph): any {
      defs = graph.defs;
      pendingLifts = [];
      seenRaw = new Set();
      try {
        return fromIdx(idx, graph.root);
      } catch (e) {
        // Restore every wire node that was lifted during this walk so the
        // decode is atomic: a later-sibling failure leaves the input wire
        // exactly as it was, so the runtime can release the owned resources.
        for (const { node, raw } of pendingLifts) {
          (node as { val: unknown }).val = raw;
        }
        throw e;
      }
    },
  };
}

/** Fused decode: flat wire `schema-value-tree` -> TS value, guided by a `ResolvedGraph`. */
export function deserializeGraphFromWit(wit: WitSchemaValueTree, graph: ResolvedGraph): any {
  preflightGraphDecode(wit.valueNodes, wit.root);
  return createWireDecoder(wit.valueNodes).readGraph(wit.root, graph);
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

    case 'secret':
      return value instanceof Secret;
    case 'quota-token':
      return value instanceof QuotaToken;
    case 'path':
      return value instanceof Path;
    case 'url':
      return value instanceof URL;
    case 'datetime':
      return value instanceof Date;
    case 'duration':
      return value instanceof Duration;
    case 'quantity':
      return value instanceof Quantity;

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

// ============================================================
// Compiled codecs
// ============================================================
// Hoists the per-node `resolveRef` + tag `switch` dispatch off the hot path by
// specializing a fixed `ResolvedGraph` once into generated JS source, compiled
// with `new Function` so QuickJS turns the whole structure into bytecode with no
// per-node dispatch. One generated function per named def handles recursion;
// everything else is inlined.
//
// The result is byte-identical to `serializeGraphToWit` on encode and value-equal
// to `deserializeGraphFromWit` on decode. The only unsupported kind is `flags`,
// which the interpreted codec does not support either; it raises
// `CompileUnsupported`, letting callers fall back to the interpreted fused codec
// (whose error message then describes the limitation).

export class CompileUnsupported extends Error {
  constructor(tag: string) {
    super(`compiled codec does not support type kind '${tag}'`);
    this.name = 'CompileUnsupported';
  }
}

// ---------------------------------------------------------------------------
// Code generation
// ---------------------------------------------------------------------------
// Emits JS source specialized to the graph and compiles it once with
// `new Function`, so QuickJS turns the whole structure into bytecode with no
// per-node dispatch and (for non-recursive structure) no per-node closure call.
// One generated function per named def handles recursion; everything else is
// inlined. Lists use `Array.prototype.map` (the QuickJS-favoured builtin).

interface GenCtx {
  fresh: () => string;
  defName: (id: TypeId) => string;
  consts: any[];
  defs: Defs;
}

function genEnc(rt: ResolvedType, valExpr: string, out: string[], ctx: GenCtx): string {
  const b = rt.body;
  switch (b.tag) {
    case 'ref':
      return `${ctx.defName(b.id)}(${valExpr}, nodes)`;
    case 'bool':
      out.push(`if (typeof ${valExpr} !== 'boolean') throw typeMismatch(${valExpr}, 'boolean');`);
      return `(nodes.push({ tag: 'bool-value', val: ${valExpr} }) - 1)`;
    case 'f32':
    case 'f64':
      out.push(`if (typeof ${valExpr} !== 'number') throw typeMismatch(${valExpr}, 'number');`);
      return `(nodes.push({ tag: '${b.tag}-value', val: ${valExpr} }) - 1)`;
    case 'u8':
    case 'u16':
    case 'u32':
    case 's8':
    case 's16':
    case 's32':
      out.push(`if (typeof ${valExpr} !== 'number') throw typeMismatch(${valExpr}, 'number');`);
      out.push(`checkIntRange('${b.tag}', ${valExpr});`);
      return `(nodes.push({ tag: '${b.tag}-value', val: ${valExpr} }) - 1)`;
    case 'u64': {
      const iv = ctx.fresh();
      out.push(`let ${iv};`);
      out.push(
        `if (typeof ${valExpr} === 'bigint') { checkBigIntRange('u64', ${valExpr}); ${iv} = nodes.push({ tag: 'u64-value', val: ${valExpr} }) - 1; } ` +
          `else if (typeof ${valExpr} === 'number') { const ${iv}b = BigInt(${valExpr}); checkBigIntRange('u64', ${iv}b); ${iv} = nodes.push({ tag: 'u64-value', val: ${iv}b }) - 1; } ` +
          `else { throw typeMismatch(${valExpr}, 'bigint'); }`,
      );
      return iv;
    }
    case 's64':
      out.push(`if (typeof ${valExpr} !== 'bigint') throw typeMismatch(${valExpr}, 'bigint');`);
      out.push(`checkBigIntRange('s64', ${valExpr});`);
      return `(nodes.push({ tag: 's64-value', val: ${valExpr} }) - 1)`;
    case 'char':
      out.push(`if (typeof ${valExpr} !== 'string') throw typeMismatch(${valExpr}, 'string');`);
      out.push(`checkCharValue(${valExpr});`);
      return `(nodes.push({ tag: 'char-value', val: ${valExpr} }) - 1)`;
    case 'string':
      out.push(`if (typeof ${valExpr} !== 'string') throw typeMismatch(${valExpr}, 'string');`);
      return `(nodes.push({ tag: 'string-value', val: ${valExpr} }) - 1)`;
    case 'enum': {
      const k = ctx.consts.push(b.cases) - 1;
      const ci = ctx.fresh();
      out.push(
        `let ${ci} = -1; if (typeof ${valExpr} === 'string') ${ci} = C[${k}].indexOf(${valExpr});`,
      );
      out.push(
        `if (${ci} === -1) throw new Error("Value '" + display(${valExpr}) + ` +
          `"' does not match any of the enum values: " + C[${k}].join(', '));`,
      );
      return `(nodes.push({ tag: 'enum-value', val: ${ci} }) - 1)`;
    }
    case 'option': {
      const ov = ctx.fresh();
      out.push(`const ${ov} = ${valExpr};`);
      const iv = ctx.fresh();
      out.push(`let ${iv};`);
      const sub: string[] = [];
      const ie = genEnc(b.element, ov, sub, ctx);
      const inner = ctx.fresh();
      out.push(
        `if (${ov} === null || ${ov} === undefined) { ${iv} = nodes.push({ tag: 'option-value', val: undefined }) - 1; } ` +
          `else { ${sub.join(' ')} const ${inner} = ${ie}; ${iv} = nodes.push({ tag: 'option-value', val: ${inner} }) - 1; }`,
      );
      return iv;
    }
    case 'list': {
      if (b.typedArray) {
        const spec = TYPED_ARRAYS[b.typedArray];
        const av = ctx.fresh();
        out.push(`const ${av} = ${valExpr};`);
        out.push(
          `if (!TYPED_ARRAYS[${JSON.stringify(b.typedArray)}].is(${av})) throw typeMismatch(${av}, ${JSON.stringify(spec.name)});`,
        );
        const idxs = ctx.fresh();
        out.push(`const ${idxs} = new Array(${av}.length);`);
        const i = ctx.fresh();
        const sub: string[] = [];
        const ee = genEnc(b.element, `${av}[${i}]`, sub, ctx);
        out.push(
          `for (let ${i} = 0; ${i} < ${av}.length; ${i}++) { ${sub.join(' ')} ${idxs}[${i}] = ${ee}; }`,
        );
        return `(nodes.push({ tag: 'list-value', val: ${idxs} }) - 1)`;
      }
      const lv = ctx.fresh();
      out.push(`const ${lv} = ${valExpr};`);
      out.push(`if (!Array.isArray(${lv})) throw typeMismatch(${lv}, 'Array');`);
      const sub: string[] = [];
      const ee = genEnc(b.element, 'x', sub, ctx);
      const cb = sub.length ? `(x) => { ${sub.join(' ')} return ${ee}; }` : `(x) => ${ee}`;
      return `(nodes.push({ tag: 'list-value', val: ${lv}.map(${cb}) }) - 1)`;
    }
    case 'record': {
      const rv = ctx.fresh();
      out.push(`const ${rv} = ${valExpr};`);
      out.push(
        `if (typeof ${rv} !== 'object' || ${rv} === null) throw typeMismatch(${rv}, 'object');`,
      );
      const idxVars: string[] = [];
      for (const f of b.fields) {
        const nameLit = JSON.stringify(f.name);
        const iv = ctx.fresh();
        if (f.type.body.tag === 'option') {
          out.push(`let ${iv};`);
          const sub: string[] = [];
          const fe = genEnc(f.type, `${rv}[${nameLit}]`, sub, ctx);
          out.push(
            `if (!Object.prototype.hasOwnProperty.call(${rv}, ${nameLit})) { ${iv} = nodes.push({ tag: 'option-value', val: undefined }) - 1; } ` +
              `else { ${sub.join(' ')} ${iv} = ${fe}; }`,
          );
        } else {
          const fe = genEnc(f.type, `${rv}[${nameLit}]`, out, ctx);
          out.push(`const ${iv} = ${fe};`);
        }
        idxVars.push(iv);
      }
      return `(nodes.push({ tag: 'record-value', val: [${idxVars.join(', ')}] }) - 1)`;
    }
    case 'tuple': {
      const tv = ctx.fresh();
      out.push(`const ${tv} = ${valExpr};`);
      if (b.empty !== undefined) {
        const iv = ctx.fresh();
        out.push(`let ${iv};`);
        out.push(
          `if (${tv} === null || ${tv} === undefined) { ${iv} = nodes.push({ tag: 'tuple-value', val: [] }) - 1; } ` +
            `else { throw typeMismatch(${tv}, 'empty tuple'); }`,
        );
        return iv;
      }
      const len = b.elements.length;
      out.push(
        `if (!Array.isArray(${tv}) || ${tv}.length !== ${len}) throw typeMismatch(${tv}, 'Array of length ${len}');`,
      );
      const idxVars: string[] = [];
      for (let i = 0; i < len; i++) {
        const fe = genEnc(b.elements[i], `${tv}[${i}]`, out, ctx);
        const iv = ctx.fresh();
        out.push(`const ${iv} = ${fe};`);
        idxVars.push(iv);
      }
      return `(nodes.push({ tag: 'tuple-value', val: [${idxVars.join(', ')}] }) - 1)`;
    }
    case 'map': {
      const mv = ctx.fresh();
      out.push(`const ${mv} = ${valExpr};`);
      out.push(`if (!(${mv} instanceof Map)) throw typeMismatch(${mv}, 'Map');`);
      const pair = ctx.fresh();
      const keySub: string[] = [];
      const keyExpr = genEnc(b.key, `${pair}[0]`, keySub, ctx);
      const ki = ctx.fresh();
      const valSub: string[] = [];
      const valExpr2 = genEnc(b.value, `${pair}[1]`, valSub, ctx);
      const vi = ctx.fresh();
      const entries = ctx.fresh();
      out.push(
        `const ${entries} = Array.from(${mv}.entries()).map((${pair}) => { ` +
          `${keySub.join(' ')} const ${ki} = ${keyExpr}; ${valSub.join(' ')} const ${vi} = ${valExpr2}; ` +
          `return { key: ${ki}, value: ${vi} }; });`,
      );
      return `(nodes.push({ tag: 'map-value', val: ${entries} }) - 1)`;
    }
    case 'variant': {
      // Each case becomes one arm of an `if / else if … else throw` chain that
      // assigns the node index to `resIdx`; never a bare `return`, which would
      // exit the enclosing generated def/root when the variant is nested.
      const vv = ctx.fresh();
      out.push(`const ${vv} = ${valExpr};`);
      const casesK = ctx.consts.push(b.cases) - 1;
      const resIdx = ctx.fresh();
      out.push(`let ${resIdx};`);
      const arms: { cond: string; body: string }[] = [];

      if (b.tagged) {
        out.push(
          `if (typeof ${vv} !== 'object' || ${vv} === null) throw typeMismatch(${vv}, 'object with tag property');`,
        );
        out.push(`if (!('tag' in ${vv})) throw missingKey('tag', ${vv});`);
        for (let i = 0; i < b.cases.length; i++) {
          const c = b.cases[i];
          const cond = `${vv}.tag === ${JSON.stringify(c.name)}`;
          if (!c.payload) {
            arms.push({
              cond,
              body: `${resIdx} = nodes.push({ tag: 'variant-value', val: { case_: ${i}, payload: undefined } }) - 1;`,
            });
            continue;
          }
          if (!c.valueKey) {
            arms.push({
              cond,
              body: `throw internalError(${JSON.stringify(`Missing payload key for tagged case ${c.name}`)});`,
            });
            continue;
          }
          const keyLit = JSON.stringify(c.valueKey);
          const sub: string[] = [];
          const pe = genEnc(c.payload, `${vv}[${keyLit}]`, sub, ctx);
          const pi = ctx.fresh();
          arms.push({
            cond,
            body:
              `if (!Object.prototype.hasOwnProperty.call(${vv}, ${keyLit})) throw missingKey(${keyLit}, ${vv}); ` +
              `${sub.join(' ')} const ${pi} = ${pe}; ` +
              `${resIdx} = nodes.push({ tag: 'variant-value', val: { case_: ${i}, payload: ${pi} } }) - 1;`,
          });
        }
      } else {
        // Plain union
        for (let i = 0; i < b.cases.length; i++) {
          const c = b.cases[i];
          if (!c.payload) {
            arms.push({
              cond: `${vv} === ${JSON.stringify(c.name)}`,
              body: `${resIdx} = nodes.push({ tag: 'variant-value', val: { case_: ${i}, payload: undefined } }) - 1;`,
            });
            continue;
          }
          const payloadK = ctx.consts.push(c.payload) - 1;
          const sub: string[] = [];
          const pe = genEnc(c.payload, vv, sub, ctx);
          const pi = ctx.fresh();
          arms.push({
            cond: `matchesResolved(${vv}, C[${payloadK}], DEFS)`,
            body: `${sub.join(' ')} const ${pi} = ${pe}; ${resIdx} = nodes.push({ tag: 'variant-value', val: { case_: ${i}, payload: ${pi} } }) - 1;`,
          });
        }
      }

      const chain = arms.map((a) => `if (${a.cond}) { ${a.body} }`).join(' else ');
      out.push(
        `${chain}${arms.length ? ' else ' : ''}{ throw unionMismatch(C[${casesK}], ${vv}); }`,
      );
      return resIdx;
    }
    case 'result': {
      const rv = ctx.fresh();
      out.push(`const ${rv} = ${valExpr};`);
      out.push(
        `if (typeof ${rv} !== 'object' || ${rv} === null) throw typeMismatch(${rv}, 'object');`,
      );
      out.push(`if (!('tag' in ${rv})) throw missingKey('tag', ${rv});`);
      const resIdx = ctx.fresh();
      out.push(`let ${resIdx};`);

      const okBranch = (): string => {
        if (b.ok) {
          if (b.repr.tag === 'custom' && !b.repr.okValueName) {
            return `{ throw internalError('unresolved key name for ok value'); }`;
          }
          const accessor =
            b.repr.tag === 'inbuilt' ? `${rv}.val` : `${rv}[${JSON.stringify(b.repr.okValueName)}]`;
          const sub: string[] = [];
          const ie = genEnc(b.ok, accessor, sub, ctx);
          const inner = ctx.fresh();
          return `{ ${sub.join(' ')} const ${inner} = ${ie}; ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'ok-value', val: ${inner} } }) - 1; }`;
        }
        if (b.repr.tag === 'inbuilt') {
          if (b.repr.okAbsent !== undefined) {
            return `{ ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'ok-value', val: undefined } }) - 1; }`;
          }
          return `{ throw internalError('unresolved ok type'); }`;
        }
        return `{ ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'ok-value', val: undefined } }) - 1; }`;
      };
      const errBranch = (): string => {
        if (b.err) {
          if (b.repr.tag === 'custom' && !b.repr.errValueName) {
            return `{ throw internalError('unresolved key name for err value'); }`;
          }
          const accessor =
            b.repr.tag === 'inbuilt'
              ? `${rv}.val`
              : `${rv}[${JSON.stringify(b.repr.errValueName)}]`;
          const sub: string[] = [];
          const ie = genEnc(b.err, accessor, sub, ctx);
          const inner = ctx.fresh();
          return `{ ${sub.join(' ')} const ${inner} = ${ie}; ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'err-value', val: ${inner} } }) - 1; }`;
        }
        if (b.repr.tag === 'inbuilt') {
          if (b.repr.errAbsent !== undefined) {
            return `{ ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'err-value', val: undefined } }) - 1; }`;
          }
          return `{ throw internalError('unresolved err type'); }`;
        }
        return `{ ${resIdx} = nodes.push({ tag: 'result-value', val: { tag: 'err-value', val: undefined } }) - 1; }`;
      };

      if (b.repr.tag === 'inbuilt') {
        out.push(
          `if (!Object.prototype.hasOwnProperty.call(${rv}, 'val')) throw missingKey('val', ${rv});`,
        );
        out.push(
          `if (${rv}.tag === 'ok') ${okBranch()} else if (${rv}.tag === 'err') ${errBranch()} else { throw typeMismatch(${rv}, 'Result'); }`,
        );
      } else {
        out.push(
          `if (${rv}.tag === 'ok') ${okBranch()} else if (${rv}.tag === 'err') ${errBranch()} else { throw typeMismatch(${rv}, 'object with tag property'); }`,
        );
      }
      return resIdx;
    }
    case 'secret': {
      const sv = ctx.fresh();
      const handle = ctx.fresh();
      const raw = ctx.fresh();
      const idx = ctx.fresh();
      out.push(`const ${sv} = ${valExpr};`);
      out.push(`if (!(${sv} instanceof Secret)) throw typeMismatch(${sv}, 'Secret');`);
      out.push(`const ${handle} = ${sv}._toSchemaValue(SECRET_INTERNAL).handle;`);
      out.push(`const ${raw} = ${handle}.withHandle((r) => r);`);
      out.push(
        `if (${raw} === undefined) throw new Error('secret handle was already transferred; an owned secret can only be sent once');`,
      );
      out.push(
        `if (__seen.has(${raw})) throw new Error('the same secret handle appeared more than once in one value tree');`,
      );
      out.push(`__seen.add(${raw});`);
      out.push(`const ${idx} = nodes.push({ tag: 'secret-value', val: undefined }) - 1;`);
      out.push(`__pending.push({ idx: ${idx}, handle: ${handle} });`);
      return idx;
    }
    case 'quota-token': {
      const qv = ctx.fresh();
      const handle = ctx.fresh();
      const raw = ctx.fresh();
      const idx = ctx.fresh();
      out.push(`const ${qv} = ${valExpr};`);
      out.push(`if (!(${qv} instanceof QuotaToken)) throw typeMismatch(${qv}, 'QuotaToken');`);
      // Peek without consuming; the take is deferred to the `__root` commit so a
      // later-sibling failure leaves the caller's token intact.
      out.push(`const ${handle} = ${qv}._toSchemaValue(QUOTA_INTERNAL).handle;`);
      out.push(`const ${raw} = ${handle}.withHandle((r) => r);`);
      out.push(
        `if (${raw} === undefined) throw new Error('quota-token handle was already transferred; an owned quota-token can only be sent once');`,
      );
      out.push(
        `if (__seen.has(${raw})) throw new Error('the same quota-token handle appeared more than once in one value tree');`,
      );
      out.push(`__seen.add(${raw});`);
      out.push(`const ${idx} = (nodes.push({ tag: 'quota-token-handle', val: undefined }) - 1);`);
      out.push(`__pending.push({ idx: ${idx}, handle: ${handle} });`);
      return idx;
    }
    case 'path':
      out.push(`if (!(${valExpr} instanceof Path)) throw typeMismatch(${valExpr}, 'Path');`);
      return `(nodes.push({ tag: 'path-value', val: ${valExpr}.path }) - 1)`;
    case 'url':
      out.push(`if (!(${valExpr} instanceof URL)) throw typeMismatch(${valExpr}, 'URL');`);
      return `(nodes.push({ tag: 'url-value', val: ${valExpr}.toString() }) - 1)`;
    case 'datetime':
      out.push(`if (!(${valExpr} instanceof Date)) throw typeMismatch(${valExpr}, 'Date');`);
      return `(nodes.push({ tag: 'datetime-value', val: dateToDatetime(${valExpr}) }) - 1)`;
    case 'duration':
      out.push(
        `if (!(${valExpr} instanceof Duration)) throw typeMismatch(${valExpr}, 'Duration');`,
      );
      return `(nodes.push({ tag: 'duration-value', val: { nanoseconds: ${valExpr}.nanoseconds } }) - 1)`;
    case 'quantity':
      out.push(
        `if (!(${valExpr} instanceof Quantity)) throw typeMismatch(${valExpr}, 'Quantity');`,
      );
      return `(nodes.push({ tag: 'quantity-value-node', val: ${valExpr}.value }) - 1)`;
    case 'flags':
      throw new CompileUnsupported(b.tag);
  }
}

function genDecRange(iv: string, out: string[]): void {
  out.push(
    `if (${iv} < 0 || ${iv} >= nodes.length) throw new SchemaDecodeError("value node index out of range: " + ${iv} + " (nodes: " + nodes.length + ")");`,
  );
}

function genDecGuardOpen(iv: string, out: string[]): void {
  genDecRange(iv, out);
  out.push(
    `if (onPath[${iv}] === 1) throw new SchemaDecodeError("cyclic value node reference at index " + ${iv});`,
  );
  out.push(`onPath[${iv}] = 1;`);
}

function genDec(rt: ResolvedType, idxExpr: string, out: string[], ctx: GenCtx): string {
  const b = rt.body;
  switch (b.tag) {
    case 'ref':
      return `${ctx.defName(b.id)}(${idxExpr}, nodes, onPath)`;
    case 'bool': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'bool-value') throw wireMismatch(${nv}, 'boolean');`);
      return `${nv}.val`;
    }
    case 'u8':
    case 'u16':
    case 'u32':
    case 's8':
    case 's16':
    case 's32':
    case 'f32':
    case 'f64': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      const rv = ctx.fresh();
      out.push(`let ${rv};`);
      out.push(
        `switch (${nv}.tag) { ` +
          `case 'u8-value': case 'u16-value': case 'u32-value': case 's8-value': case 's16-value': case 's32-value': case 'f32-value': case 'f64-value': ${rv} = ${nv}.val; break; ` +
          `case 'u64-value': case 's64-value': ${rv} = Number(${nv}.val); break; ` +
          `default: throw wireMismatch(${nv}, 'number'); }`,
      );
      return rv;
    }
    case 'u64':
    case 's64': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      const rv = ctx.fresh();
      out.push(`let ${rv};`);
      out.push(
        `if (${nv}.tag === 'u64-value' || ${nv}.tag === 's64-value') ${rv} = ${nv}.val; else throw wireMismatch(${nv}, 'bigint');`,
      );
      return rv;
    }
    case 'char': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'char-value') throw wireMismatch(${nv}, 'char');`);
      return `${nv}.val`;
    }
    case 'string': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'string-value') throw wireMismatch(${nv}, 'string');`);
      return `${nv}.val`;
    }
    case 'enum': {
      const k = ctx.consts.push(b.cases) - 1;
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'enum-value') throw wireMismatch(${nv}, 'enum');`);
      out.push(
        `if (!Number.isInteger(${nv}.val) || ${nv}.val < 0 || ${nv}.val >= C[${k}].length) throw wireMismatch(${nv}, 'enum');`,
      );
      return `C[${k}][${nv}.val]`;
    }
    case 'option': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'option-value') throw wireMismatch(${nv}, 'option');`);
      const rv = ctx.fresh();
      out.push(`let ${rv};`);
      const sub: string[] = [];
      const ie = genDec(b.element, `${nv}.val`, sub, ctx);
      const noneExpr = b.noneRepr === 'null' ? 'null' : 'undefined';
      out.push(
        `if (${nv}.val === undefined) { ${rv} = ${noneExpr}; } else { ${sub.join(' ')} ${rv} = ${ie}; }`,
      );
      out.push(`onPath[${iv}] = 0;`);
      return rv;
    }
    case 'list': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'list-value') throw wireMismatch(${nv}, 'list');`);
      if (b.typedArray) {
        const arr = ctx.fresh();
        out.push(
          `const ${arr} = TYPED_ARRAYS[${JSON.stringify(b.typedArray)}].make(${nv}.val.length);`,
        );
        const i = ctx.fresh();
        const sub: string[] = [];
        const ee = genDec(b.element, `${nv}.val[${i}]`, sub, ctx);
        out.push(
          `for (let ${i} = 0; ${i} < ${nv}.val.length; ${i}++) { ${sub.join(' ')} ${arr}[${i}] = ${ee}; }`,
        );
        out.push(`onPath[${iv}] = 0;`);
        return arr;
      }
      const sub: string[] = [];
      const ee = genDec(b.element, 'ei', sub, ctx);
      const cb = sub.length ? `(ei) => { ${sub.join(' ')} return ${ee}; }` : `(ei) => ${ee}`;
      const rv = ctx.fresh();
      out.push(`const ${rv} = ${nv}.val.map(${cb});`);
      out.push(`onPath[${iv}] = 0;`);
      return rv;
    }
    case 'record': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'record-value') throw wireMismatch(${nv}, 'record');`);
      out.push(`if (${nv}.val.length !== ${b.fields.length}) throw wireMismatch(${nv}, 'record');`);
      const ov = ctx.fresh();
      out.push(`const ${ov} = {};`);
      for (let i = 0; i < b.fields.length; i++) {
        const fe = genDec(b.fields[i].type, `${nv}.val[${i}]`, out, ctx);
        out.push(`${ov}[${JSON.stringify(b.fields[i].name)}] = ${fe};`);
      }
      out.push(`onPath[${iv}] = 0;`);
      return ov;
    }
    case 'tuple': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      if (b.empty !== undefined) {
        genDecRange(iv, out);
        const nv = ctx.fresh();
        out.push(`const ${nv} = nodes[${iv}];`);
        out.push(`if (${nv}.tag !== 'tuple-value') throw wireMismatch(${nv}, 'tuple');`);
        out.push(`if (${nv}.val.length !== 0) throw wireMismatch(${nv}, 'empty tuple');`);
        return b.empty === 'null' ? 'null' : 'undefined';
      }
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'tuple-value') throw wireMismatch(${nv}, 'tuple');`);
      const len = b.elements.length;
      out.push(`if (${nv}.val.length !== ${len}) throw wireMismatch(${nv}, 'tuple');`);
      const ov = ctx.fresh();
      out.push(`const ${ov} = new Array(${len});`);
      for (let i = 0; i < len; i++) {
        const fe = genDec(b.elements[i], `${nv}.val[${i}]`, out, ctx);
        out.push(`${ov}[${i}] = ${fe};`);
      }
      out.push(`onPath[${iv}] = 0;`);
      return ov;
    }
    case 'map': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'map-value') throw wireMismatch(${nv}, 'map');`);
      const mapv = ctx.fresh();
      out.push(`const ${mapv} = new Map();`);
      const entry = ctx.fresh();
      const keySub: string[] = [];
      const keyExpr = genDec(b.key, `${entry}.key`, keySub, ctx);
      const ki = ctx.fresh();
      const valSub: string[] = [];
      const valExpr2 = genDec(b.value, `${entry}.value`, valSub, ctx);
      const vi = ctx.fresh();
      out.push(
        `for (const ${entry} of ${nv}.val) { ` +
          `${keySub.join(' ')} const ${ki} = ${keyExpr}; ${valSub.join(' ')} const ${vi} = ${valExpr2}; ` +
          `${mapv}.set(${ki}, ${vi}); }`,
      );
      out.push(`onPath[${iv}] = 0;`);
      return mapv;
    }
    case 'variant': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'variant-value') throw wireMismatch(${nv}, 'variant');`);
      const vv = ctx.fresh();
      out.push(`const ${vv} = ${nv}.val;`);
      const resVar = ctx.fresh();
      out.push(`let ${resVar};`);
      out.push(`switch (${vv}.case_) {`);
      for (let i = 0; i < b.cases.length; i++) {
        const c = b.cases[i];
        const nameLit = JSON.stringify(c.name);
        if (b.tagged) {
          if (!c.payload) {
            out.push(
              `case ${i}: { if (${vv}.payload !== undefined) throw wireMismatch(${nv}, 'variant'); ${resVar} = { tag: ${nameLit} }; break; }`,
            );
            continue;
          }
          const keyLit = c.valueKey === undefined ? undefined : JSON.stringify(c.valueKey);
          const sub: string[] = [];
          const pe = genDec(c.payload, `${vv}.payload`, sub, ctx);
          const undefinedCase = `throw wireMismatch(${nv}, 'variant');`;
          const presentCase =
            keyLit === undefined
              ? `throw wireMismatch(${nv}, 'variant');`
              : `${sub.join(' ')} ${resVar} = { tag: ${nameLit}, [${keyLit}]: ${pe} };`;
          out.push(
            `case ${i}: { if (${vv}.payload === undefined) { ${undefinedCase} } else { ${presentCase} } break; }`,
          );
          continue;
        }
        // Plain union
        if (!c.payload) {
          out.push(
            `case ${i}: { if (${vv}.payload !== undefined) throw wireMismatch(${nv}, 'variant'); ${resVar} = ${nameLit}; break; }`,
          );
          continue;
        }
        const sub: string[] = [];
        const pe = genDec(c.payload, `${vv}.payload`, sub, ctx);
        out.push(
          `case ${i}: { if (${vv}.payload === undefined) throw wireMismatch(${nv}, 'variant'); ` +
            `${sub.join(' ')} ${resVar} = ${pe}; break; }`,
        );
      }
      out.push(`default: throw wireMismatch(${nv}, 'variant'); }`);
      out.push(`onPath[${iv}] = 0;`);
      return resVar;
    }
    case 'result': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecGuardOpen(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'result-value') throw wireMismatch(${nv}, 'result');`);
      const rr = ctx.fresh();
      out.push(`const ${rr} = ${nv}.val;`);
      out.push(
        `if (${rr}.tag !== 'ok-value' && ${rr}.tag !== 'err-value') throw new SchemaDecodeError("unknown result value payload tag '" + ${rr}.tag + "'");`,
      );
      const isOk = ctx.fresh();
      const hasVal = ctx.fresh();
      out.push(`const ${isOk} = ${rr}.tag === 'ok-value';`);
      out.push(`const ${hasVal} = ${rr}.val !== undefined;`);
      const resVar = ctx.fresh();
      const done = ctx.fresh();
      out.push(`let ${resVar}; let ${done} = false;`);

      if (b.repr.tag === 'inbuilt') {
        const okStmts: string[] = [];
        if (b.ok) {
          const sub: string[] = [];
          const ie = genDec(b.ok, `${rr}.val`, sub, ctx);
          okStmts.push(
            `if (${hasVal}) { ${sub.join(' ')} ${resVar} = Result.ok(${ie}); ${done} = true; }`,
          );
        }
        if (b.repr.okAbsent !== undefined) {
          const absent = b.repr.okAbsent === 'null' ? 'null' : 'undefined';
          okStmts.push(`if (!${hasVal}) { ${resVar} = Result.ok(${absent}); ${done} = true; }`);
        }
        const errStmts: string[] = [];
        if (b.err) {
          const sub: string[] = [];
          const ie = genDec(b.err, `${rr}.val`, sub, ctx);
          errStmts.push(
            `if (${hasVal}) { ${sub.join(' ')} ${resVar} = Result.err(${ie}); ${done} = true; }`,
          );
        }
        if (b.repr.errAbsent !== undefined) {
          const absent = b.repr.errAbsent === 'null' ? 'null' : 'undefined';
          errStmts.push(`if (!${hasVal}) { ${resVar} = Result.err(${absent}); ${done} = true; }`);
        }
        out.push(`if (${isOk}) { ${okStmts.join(' ')} } else { ${errStmts.join(' ')} }`);
      } else {
        // custom repr
        const okName = b.repr.okValueName;
        const errName = b.repr.errValueName;
        const hasOk = !!b.ok;
        const hasErr = !!b.err;
        const okNameLit = okName === undefined ? undefined : JSON.stringify(okName);
        const errNameLit = errName === undefined ? undefined : JSON.stringify(errName);
        if (okName && errName && hasOk && hasErr) {
          const okSub: string[] = [];
          const okE = genDec(b.ok!, `${rr}.val`, okSub, ctx);
          const errSub: string[] = [];
          const errE = genDec(b.err!, `${rr}.val`, errSub, ctx);
          out.push(
            `if (${isOk} && ${hasVal}) { ${okSub.join(' ')} ${resVar} = { tag: 'ok', [${okNameLit}]: ${okE} }; ${done} = true; } ` +
              `else if (!${isOk} && ${hasVal}) { ${errSub.join(' ')} ${resVar} = { tag: 'err', [${errNameLit}]: ${errE} }; ${done} = true; }`,
          );
        } else if (okName && hasOk && !hasErr) {
          const okSub: string[] = [];
          const okE = genDec(b.ok!, `${rr}.val`, okSub, ctx);
          out.push(
            `if (${isOk} && ${hasVal}) { ${okSub.join(' ')} ${resVar} = { tag: 'ok', [${okNameLit}]: ${okE} }; ${done} = true; } ` +
              `else if (!${isOk} && !${hasVal}) { ${resVar} = { tag: 'err' }; ${done} = true; }`,
          );
        } else if (errName && hasErr && !hasOk) {
          const errSub: string[] = [];
          const errE = genDec(b.err!, `${rr}.val`, errSub, ctx);
          out.push(
            `if (!${isOk} && ${hasVal}) { ${errSub.join(' ')} ${resVar} = { tag: 'err', [${errNameLit}]: ${errE} }; ${done} = true; } ` +
              `else if (${isOk} && !${hasVal}) { ${resVar} = { tag: 'ok' }; ${done} = true; }`,
          );
        } else {
          if (okName && !hasOk) {
            out.push(
              `if (${isOk} && !${hasVal}) { ${resVar} = { tag: 'ok', [${okNameLit}]: undefined }; ${done} = true; }`,
            );
          }
          if (errName && !hasErr) {
            out.push(
              `if (!${isOk} && !${hasVal}) { ${resVar} = { tag: 'err', [${errNameLit}]: undefined }; ${done} = true; }`,
            );
          }
        }
      }

      out.push(`if (!${done}) throw wireMismatch(${nv}, 'result');`);
      out.push(`onPath[${iv}] = 0;`);
      return resVar;
    }
    case 'secret': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'secret-value') throw wireMismatch(${nv}, 'Secret');`);
      const raw = ctx.fresh();
      out.push(`const ${raw} = ${nv}.val;`);
      out.push(
        `if (${raw} === undefined) throw new SchemaDecodeError('secret handle referenced more than once');`,
      );
      out.push(
        `if (__seen.has(${raw})) throw new SchemaDecodeError('the same secret resource appeared more than once');`,
      );
      out.push(`__seen.add(${raw});`);
      out.push(`${nv}.val = undefined;`);
      out.push(`__pending.push({ idx: ${iv}, raw: ${raw} });`);
      const graphIdx = ctx.consts.push({ defs: ctx.defs, root: b.inner }) - 1;
      return `Secret._fromHandle(SECRET_INTERNAL, GuestSecretHandle.fromRaw(SECRET_INTERNAL, ${raw}), C[${graphIdx}])`;
    }
    case 'quota-token': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      out.push(`if (${nv}.tag !== 'quota-token-handle') throw wireMismatch(${nv}, 'QuotaToken');`);
      const raw = ctx.fresh();
      out.push(`const ${raw} = ${nv}.val;`);
      out.push(
        `if (${raw} === undefined) throw new SchemaDecodeError('quota-token handle referenced more than once');`,
      );
      out.push(
        `if (__seen.has(${raw})) throw new SchemaDecodeError('the same quota-token resource appeared more than once');`,
      );
      out.push(`__seen.add(${raw});`);
      // Lift the owned resource out of the wire node, but record the lift so the
      // `__root` wrapper can restore the wire on a later-sibling failure.
      out.push(`${nv}.val = undefined;`);
      out.push(`__pending.push({ idx: ${iv}, raw: ${raw} });`);
      return `QuotaToken._fromHandle(QUOTA_INTERNAL, GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, ${raw}))`;
    }
    case 'path':
    case 'url':
    case 'datetime':
    case 'duration':
    case 'quantity': {
      const iv = ctx.fresh();
      out.push(`const ${iv} = ${idxExpr};`);
      genDecRange(iv, out);
      const nv = ctx.fresh();
      out.push(`const ${nv} = nodes[${iv}];`);
      if (b.tag === 'path') {
        out.push(`if (${nv}.tag !== 'path-value') throw wireMismatch(${nv}, 'Path');`);
        return `new Path(${nv}.val)`;
      }
      if (b.tag === 'url') {
        out.push(`if (${nv}.tag !== 'url-value') throw wireMismatch(${nv}, 'URL');`);
        return `new URL(${nv}.val)`;
      }
      if (b.tag === 'datetime') {
        out.push(`if (${nv}.tag !== 'datetime-value') throw wireMismatch(${nv}, 'Date');`);
        return `datetimeToDate(${nv}.val)`;
      }
      if (b.tag === 'duration') {
        out.push(`if (${nv}.tag !== 'duration-value') throw wireMismatch(${nv}, 'Duration');`);
        return `new Duration(${nv}.val.nanoseconds)`;
      }
      out.push(`if (${nv}.tag !== 'quantity-value-node') throw wireMismatch(${nv}, 'Quantity');`);
      return `new Quantity(${nv}.val)`;
    }
    case 'flags':
      throw new CompileUnsupported(b.tag);
  }
}

function makeGenCtx(graph: ResolvedGraph, prefix: string): GenCtx {
  let counter = 0;
  const defIndex = new Map<TypeId, number>();
  let di = 0;
  for (const id of graph.defs.keys()) defIndex.set(id, di++);
  return {
    fresh: () => `t${counter++}`,
    defName: (id: TypeId) => {
      const k = defIndex.get(id);
      if (k === undefined) throw internalError(`unknown def '${id}'`);
      return `${prefix}${k}`;
    },
    consts: [],
    defs: graph.defs,
  };
}

/**
 * Generate and compile a node-level encoder for the graph. The
 * returned function emits `value` into a caller-provided `nodes` pool and returns
 * the root node index, so it can serve both the standalone single-root case and
 * the boundary's multi-field shared-pool fusion. Throws {@link CompileUnsupported}
 * during code generation (before `new Function`) for unsupported kinds.
 */
function genGraphEmitFn(
  graph: ResolvedGraph,
): (v: any, nodes: WitSchemaValueNode[]) => ValueNodeIndex {
  const ctx = makeGenCtx(graph, 'e');
  // `__pending` / `__seen` are shared across the generated `__root` and def
  // functions (all defined in this one `new Function` body, so they close over
  // the same bindings). `__root` resets them per call and commits the deferred
  // quota-token takes only after a successful walk — atomic, matching the
  // non-fused `schemaValueToWit` preflight. See the `quota-token` case in
  // `genEnc` for the peek-and-defer.
  const lines: string[] = ['"use strict";', 'let __pending; let __seen;'];
  for (const [id, def] of graph.defs) {
    const body: string[] = [];
    const expr = genEnc(def, 'v', body, ctx);
    lines.push(`const ${ctx.defName(id)} = (v, nodes) => {`, ...body, `return ${expr};`, `};`);
  }
  const rootBody: string[] = [];
  const rootExpr = genEnc(graph.root, 'v', rootBody, ctx);
  lines.push(
    `const __root = (v, nodes) => {`,
    `__pending = []; __seen = new Set();`,
    ...rootBody,
    `const __rootIdx = ${rootExpr};`,
    // Commit every deferred take now that the whole walk succeeded. `take()`
    // cannot return undefined here: the peek confirmed presence/uniqueness and
    // nothing else moves the handle on this thread.
    `for (const __p of __pending) { const __raw = __p.handle.take(); if (__raw === undefined) throw new Error('owned handle was already transferred; an owned resource can only be sent once'); nodes[__p.idx].val = __raw; }`,
    `return __rootIdx;`,
    `};`,
    `return __root;`,
  );
  const factory = new Function(
    'typeMismatch',
    'checkIntRange',
    'checkBigIntRange',
    'checkCharValue',
    'missingKey',
    'unionMismatch',
    'internalError',
    'matchesResolved',
    'display',
    'TYPED_ARRAYS',
    'Secret',
    'SECRET_INTERNAL',
    'QuotaToken',
    'QUOTA_INTERNAL',
    'Path',
    'Duration',
    'Quantity',
    'dateToDatetime',
    'DEFS',
    'C',
    lines.join('\n'),
  ) as (...a: any[]) => (v: any, nodes: WitSchemaValueNode[]) => ValueNodeIndex;
  return factory(
    typeMismatch,
    checkIntRange,
    checkBigIntRange,
    checkCharValue,
    missingKey,
    unionMismatch,
    internalError,
    matchesResolved,
    display,
    TYPED_ARRAYS,
    Secret,
    SECRET_INTERNAL,
    QuotaToken,
    QUOTA_INTERNAL,
    Path,
    Duration,
    Quantity,
    dateToDatetime,
    graph.defs,
    ctx.consts,
  );
}

/**
 * Generate and compile a node-level decoder for the graph. The
 * returned function reads the value at `idx` from a caller-provided `nodes` pool
 * using a caller-provided `onPath` cycle guard, so it can serve both the
 * standalone single-root case and the boundary's multi-field shared-pool fusion.
 * Throws {@link CompileUnsupported} during code generation for unsupported kinds.
 */
function genGraphReadFn(
  graph: ResolvedGraph,
): (idx: ValueNodeIndex, nodes: WitSchemaValueNode[], onPath: Uint8Array) => any {
  const ctx = makeGenCtx(graph, 'd');
  // `__pending` / `__seen` are shared across the generated `__root` and def
  // functions (all defined in this one `new Function` body). `__root` resets them
  // per call and, on a thrown error, restores every wire node that was lifted
  // during the walk — atomic, matching the non-fused `schemaValueFromWit`
  // preflight. See the `quota-token` case in `genDec` for the lift-and-record.
  const lines: string[] = ['"use strict";', 'let __pending; let __seen;'];
  for (const [id, def] of graph.defs) {
    const body: string[] = [];
    const expr = genDec(def, 'idx', body, ctx);
    lines.push(
      `const ${ctx.defName(id)} = (idx, nodes, onPath) => {`,
      ...body,
      `return ${expr};`,
      `};`,
    );
  }
  const rootBody: string[] = [];
  const rootExpr = genDec(graph.root, 'idx', rootBody, ctx);
  lines.push(
    `const __root = (idx, nodes, onPath) => {`,
    `__pending = []; __seen = new Set();`,
    `try {`,
    ...rootBody,
    `return ${rootExpr};`,
    `} catch (__e) {`,
    // Restore every wire node lifted during this walk so a later-sibling
    // failure leaves the input wire exactly as it was, for the runtime to
    // release the owned resources.
    `for (const __p of __pending) { nodes[__p.idx].val = __p.raw; }`,
    `onPath.fill(0);`,
    `throw __e;`,
    `}`,
    `};`,
    `return __root;`,
  );
  const factory = new Function(
    'wireMismatch',
    'SchemaDecodeError',
    'Result',
    'TYPED_ARRAYS',
    'Secret',
    'GuestSecretHandle',
    'SECRET_INTERNAL',
    'QuotaToken',
    'GuestQuotaTokenHandle',
    'QUOTA_INTERNAL',
    'Path',
    'Duration',
    'Quantity',
    'datetimeToDate',
    'C',
    lines.join('\n'),
  ) as (
    ...a: any[]
  ) => (idx: ValueNodeIndex, nodes: WitSchemaValueNode[], onPath: Uint8Array) => any;
  return factory(
    wireMismatch,
    SchemaDecodeError,
    Result,
    TYPED_ARRAYS,
    Secret,
    GuestSecretHandle,
    SECRET_INTERNAL,
    QuotaToken,
    GuestQuotaTokenHandle,
    QUOTA_INTERNAL,
    Path,
    Duration,
    Quantity,
    datetimeToDate,
    ctx.consts,
  );
}

/** Generate and compile a standalone source-level encoder for the graph. */
export function compileGraphEncoder(graph: ResolvedGraph): (value: any) => WitSchemaValueTree {
  const rootFn = genGraphEmitFn(graph);
  return (value) => {
    const nodes: WitSchemaValueNode[] = [];
    const r = rootFn(value, nodes);
    return { valueNodes: nodes, root: r };
  };
}

/** Generate and compile a standalone source-level decoder for the graph. */
export function compileGraphDecoder(graph: ResolvedGraph): (wit: WitSchemaValueTree) => any {
  const rootFn = genGraphReadFn(graph);
  return (wit) => {
    preflightGraphDecode(wit.valueNodes, wit.root);
    const onPath = new Uint8Array(wit.valueNodes.length);
    return rootFn(wit.root, wit.valueNodes, onPath);
  };
}

function preflightGraphDecode(nodes: WitSchemaValueNode[], root: ValueNodeIndex): void {
  try {
    preflightWitValueTree(nodes, root);
  } catch (e) {
    drainUnconsumedQuotaHandles(nodes);
    throw e;
  }
}

// ---------------------------------------------------------------------------
// Compiled codec cache (production integration)
// ---------------------------------------------------------------------------
// A per-graph compiled codec that the runtime value boundary uses on
// the hot invocation / RPC paths. `emit` / `read` operate on a caller-provided
// node pool so they slot into the boundary's multi-field input-record fusion;
// `encode` / `decode` are standalone single-value wrappers for method outputs.

export interface GraphCodec {
  /** Emit a TS value against this graph into a shared node pool; returns its node index. */
  emit(value: any, nodes: WitSchemaValueNode[]): ValueNodeIndex;
  /** Read this graph's value at `idx` from a shared node pool, guarded by `onPath`. */
  read(idx: ValueNodeIndex, nodes: WitSchemaValueNode[], onPath: Uint8Array): any;
  /** Standalone encode: TS value -> wire tree. */
  encode(value: any): WitSchemaValueTree;
  /** Standalone decode: wire tree -> TS value. */
  decode(wit: WitSchemaValueTree): any;
}

function buildGraphCodec(graph: ResolvedGraph): GraphCodec {
  // May throw CompileUnsupported during code generation, before `new Function`.
  const emit = genGraphEmitFn(graph);
  const read = genGraphReadFn(graph);
  return {
    emit,
    read,
    encode(value) {
      const nodes: WitSchemaValueNode[] = [];
      const root = emit(value, nodes);
      return { valueNodes: nodes, root };
    },
    decode(wit) {
      preflightGraphDecode(wit.valueNodes, wit.root);
      const onPath = new Uint8Array(wit.valueNodes.length);
      return read(wit.root, wit.valueNodes, onPath);
    },
  };
}

// `null` marks a graph the compiler cannot or should not specialize (unsupported
// kind, or a `new Function` failure): callers then use the interpreted fused
// codec. Keyed by graph identity, which the registries keep stable between
// agent-registration time and invocation. A `WeakMap` so codecs are collected
// with their graphs.
const graphCodecCache = new WeakMap<ResolvedGraph, GraphCodec | null>();

/**
 * Return a compiled codec for `graph`, or `null` if the graph cannot
 * be specialized — in which case callers fall back to the interpreted fused
 * codec. Results are cached per graph.
 *
 * Call this eagerly at agent-registration time (which runs during top-level
 * module evaluation). The compiled function objects then become part of the
 * Wizer pre-initialization snapshot captured by `golem build`, so the one-time
 * `new Function` compilation cost is paid at build time and adds no per-process
 * startup cost. A lazy first-invocation call still works but pays that cost at
 * runtime.
 */
export function getGraphCodec(graph: ResolvedGraph): GraphCodec | null {
  const cached = graphCodecCache.get(graph);
  if (cached !== undefined) return cached;
  let codec: GraphCodec | null;
  try {
    codec = buildGraphCodec(graph);
  } catch {
    // Unsupported kind or codegen/compile failure: fall back to the interpreted
    // fused codec, which is always correct.
    codec = null;
  }
  graphCodecCache.set(graph, codec);
  return codec;
}
