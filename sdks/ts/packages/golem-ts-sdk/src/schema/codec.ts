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

// `SchemaCodec` pairs a schema's WIT type with its value codec: walking a schema
// once yields BOTH the WIT type (`SchemaGraph`) and the bidirectional value codec
// (`toValue`/`fromValue`). It depends only on the new schema model
// (`internal/schema-model/`), never on the decorator-era `Type.Type` resolvers.

import {
  cloneSchemaValue,
  deepEqual,
  schemaValueToWit,
  SchemaGraph,
  SchemaType,
  SchemaValue,
  freezeSchemaValue,
  schemaGraphToWit,
  numericRestrictionsMatch,
} from '../internal/schema-model';
import {
  createUntrackedGuestSecretHandle,
  peekGuestSecretHandle,
  releaseGuestSecretHandle,
  takeGuestSecretHandle,
} from '../internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../internal/schema-model/secretInternal';
import {
  createUntrackedGuestQuotaTokenHandle,
  peekGuestQuotaTokenHandle,
  releaseGuestQuotaTokenHandle,
  takeGuestQuotaTokenHandle,
} from '../internal/schema-model/quotaTokenHandle';
import { QUOTA_INTERNAL } from '../internal/schema-model/quotaInternal';
import {
  createUntrackedGuestPermissionCardHandle,
  peekGuestPermissionCardHandle,
  releaseGuestPermissionCardHandle,
  takeGuestPermissionCardHandle,
} from '../internal/schema-model/permissionCardHandle';
import { PERMISSION_CARD_INTERNAL } from '../internal/schema-model/permissionCardInternal';
import type {
  PermissionCard as RawPermissionCard,
  QuotaToken as RawQuotaToken,
  SchemaValueNode as WireValueNode,
  SchemaValueTree as WireValueTree,
  Secret as RawSecret,
} from 'golem:core/types@2.0.0';
import { Result } from '../host/result';
import type { StandardSchemaV1 } from './standardSchema';

/** An SDK codec rejected a value because its outer source shape does not match. */
export class CodecShapeMismatchError extends TypeError {}

/**
 * A stable schema graph paired with deterministic value conversions. Codecs are
 * immutable once compiled: conversions must not mutate the codec, its graph, or
 * any codec reachable through structural child links.
 */
export interface SchemaCodec {
  /** Immutable root SchemaType and the nominal definitions it references. */
  readonly graph: SchemaGraph;
  readonly toValue: (value: unknown) => SchemaValue;
  readonly fromValue: (value: SchemaValue) => unknown;
  /** Direct flat-wire conversion. Absent when this codec can contain owned resources. */
  readonly direct?: DirectSchemaCodec;
  /** Source-shape information consumed by the static component codec emitter. */
  readonly concrete?: ConcreteCodecMetadata;
  /** Source validator retained for metadata literals whose constraints are not representable in WIT. */
  readonly sourceSchema?: StandardSchemaV1;
  /**
   * True for the unit/void type: the method's `returns` maps to WIT
   * `output-schema.unit`, so `graph` is a placeholder and is never encoded.
   */
  readonly isUnit?: boolean;
  /**
   * For OBJECT codecs (a WIT `record` with named fields, e.g. `z.object({...})`):
   * the per-field child codecs, in declaration order. Set by the vendor object
   * walkers so the config surface can flatten nested config to leaf fields
   * (each fetched by its full multi-segment path). Absent for non-object codecs
   * (including `z.record(k, v)` maps, which are read whole).
   */
  readonly fields?: ReadonlyArray<{ readonly name: string; readonly codec: SchemaCodec }>;
  /** JavaScript absence convention for a codec whose root is a WIT `option`. */
  readonly optionKind?: 'optional' | 'nullable' | 'nullish';
  /**
   * For an OPTIONAL object group (`z.object({...}).optional()`): the codec's own
   * `graph` round-trips as `option<record>`, but {@link fields} is ALSO exposed
   * (mirrored from the inner object) so the config surface can DESCEND the group
   * into per-leaf declarations. This flag tells the config surface that the
   * descended group is optional — its leaves are declared to the host as
   * `option<leaf>` (so an unset leaf reads as option-none instead of trapping)
   * and its runtime presence is decided by its REQUIRED children. Absent for a
   * plain (non-optional) object group.
   */
  readonly optionalGroup?: boolean;
  /** Inner codec for a WIT `option`, preserving the source-schema convention. */
  readonly optionInner?: SchemaCodec;
  /** Item codec for a WIT `list` or `fixed-list`. */
  readonly listItem?: SchemaCodec;
  /** Item codec for a typed schema-value stream. */
  readonly streamItem?: SchemaCodec;
  /** Child codecs for a WIT `map`, when the source schema exposes them. */
  readonly mapKey?: SchemaCodec;
  readonly mapValue?: SchemaCodec;
  /** Arms for a WIT result. */
  readonly resultOk?: SchemaCodec;
  readonly resultErr?: SchemaCodec;
  /**
   * For SECRET markers (`s.secret(inner)`): the inner (revealed-value) codec —
   * the one that decodes the plaintext after `golem:secrets/reveal`. The
   * marker's own `graph` is `secret<inner>` and its own `fromValue` yields the
   * raw handle; the config surface uses this inner codec to decode a revealed
   * secret leaf.
   */
  readonly secretInner?: SchemaCodec;
  /**
   * For the PRINCIPAL marker (`s.principal()`): the auto-injection kind. When a
   * method/constructor takes a bare `s.principal()` parameter, the caller does
   * NOT supply it — the host injects the caller's `Principal` (WIT
   * `field-source.auto-injected(principal)`). The runtime uses this to emit the
   * `auto-injected` source, decode the param from the separate invoke `principal`
   * arg (consuming no wire field), and exclude it from HTTP/RPC caller inputs.
   * A principal NESTED inside a record/return is ordinary user-supplied data and
   * is unaffected (only a top-level parameter codec is auto-injected).
   */
  readonly autoInjected?: 'principal';
}

export type ConcreteCodecMetadata =
  | { readonly tag: 'typed-array'; readonly constructor: string }
  | {
      readonly tag: 'variant';
      readonly cases: ReadonlyArray<{ readonly codec?: SchemaCodec; readonly value?: unknown }>;
      readonly discriminator?: string;
      readonly sourceTag?: string;
    }
  | {
      readonly tag: 'multimodal';
      readonly cases: ReadonlyArray<{ name: string; codec: SchemaCodec }>;
    }
  | { readonly tag: 'unstructured-text' | 'unstructured-binary' }
  | { readonly tag: 'principal' };

/** Shared flat-value arena used while directly encoding child codecs. */
export class SchemaValueWriter {
  readonly valueNodes: WireValueNode[] = [];

  add(node: WireValueNode): number {
    this.valueNodes.push(node);
    return this.valueNodes.length - 1;
  }
}

/** Shared flat-value arena used while directly decoding child codecs. */
export class SchemaValueReader {
  private readonly active = new Set<number>();
  private readonly read = new Set<number>();

  constructor(readonly valueNodes: readonly WireValueNode[]) {}

  node<T>(
    index: number | undefined,
    tag: WireValueNode['tag'],
    decode: (node: WireValueNode) => T,
  ): T {
    if (
      typeof index !== 'number' ||
      !Number.isInteger(index) ||
      index < 0 ||
      index >= this.valueNodes.length
    ) {
      throw new TypeError(
        `value node index out of range: ${index} (nodes: ${this.valueNodes.length})`,
      );
    }
    if (this.active.has(index)) throw new TypeError(`cycle at value node index ${index}`);
    if (this.read.has(index)) throw new TypeError(`aliased value node index ${index}`);
    const node = this.valueNodes[index]!;
    if (node.tag !== tag)
      throw new TypeError(`expected ${tag} at value node index ${index}, got ${node.tag}`);
    this.active.add(index);
    this.read.add(index);
    try {
      return decode(node);
    } finally {
      this.active.delete(index);
    }
  }

  finish(): void {
    if (this.read.size !== this.valueNodes.length) {
      throw new TypeError('flat value tree contains unreachable value nodes');
    }
  }
}

export interface DirectSchemaCodec {
  write(value: unknown, writer: SchemaValueWriter): number | undefined;
  read(reader: SchemaValueReader, index: number | undefined): unknown;
}

export function directSchemaValueToWit(codec: SchemaCodec, value: unknown): WireValueTree {
  if (!codec.direct)
    throw new TypeError('schema codec does not support direct flat-wire conversion');
  const writer = new SchemaValueWriter();
  const root = codec.direct.write(value, writer);
  if (root === undefined) throw new TypeError('unit has no standalone flat-wire value');
  const tree = { valueNodes: writer.valueNodes, root };
  if (!deepEqual(directSchemaValueFromWit(codec, tree), value)) {
    throw new TypeError('is not canonical for its declared schema');
  }
  return tree;
}

export function directSchemaValueFromWit(codec: SchemaCodec, tree: WireValueTree): unknown {
  if (!codec.direct)
    throw new TypeError('schema codec does not support direct flat-wire conversion');
  const reader = new SchemaValueReader(tree.valueNodes);
  const value = codec.direct.read(reader, tree.root);
  reader.finish();
  return value;
}

/** Encode a concrete value directly into its typed wire carrier. */
export function directTypedSchemaValueToWit(codec: SchemaCodec, value: unknown) {
  return { graph: schemaGraphToWit(codec.graph), value: directSchemaValueToWit(codec, value) };
}

/** Install direct operations from the explicit child links produced by schema constructors. */
export function withDirectCodec(codec: SchemaCodec): SchemaCodec {
  const direct = buildDirect(codec);
  return direct ? { ...codec, direct } : codec;
}

function buildDirect(codec: SchemaCodec): DirectSchemaCodec | undefined {
  const child = (value: SchemaCodec | undefined) => {
    if (!value) return undefined;
    if (value.direct) return value.direct;
    return buildDirect(value);
  };
  if (codec.isUnit) {
    return {
      write: (value) => {
        if (value !== undefined) throw new TypeError('unit value must be undefined');
        return undefined;
      },
      read: (_reader, index) => {
        if (index !== undefined) throw new TypeError('unit result arm has an unexpected payload');
        return undefined;
      },
    };
  }
  if (codec.fields && !codec.optionInner) {
    const fields = codec.fields.map((field) => ({ name: field.name, direct: child(field.codec) }));
    if (fields.some((field) => !field.direct)) return undefined;
    return {
      write: (value, writer) =>
        writer.add({
          tag: 'record-value',
          val: fields.map((field) =>
            requiredIndex(
              field.direct!.write((value as Record<string, unknown>)[field.name], writer),
              'record field',
            ),
          ),
        }),
      read: (reader, index) =>
        reader.node(index, 'record-value', (node) => {
          const indices = (node as Extract<WireValueNode, { tag: 'record-value' }>).val;
          if (indices.length !== fields.length)
            throw new TypeError('record field count does not match schema');
          return Object.fromEntries(
            fields.map((field, i) => [field.name, field.direct!.read(reader, indices[i]!)]),
          );
        }),
    };
  }
  if (codec.listItem) {
    const item = child(codec.listItem);
    if (!item) return undefined;
    return {
      write: (value, writer) =>
        writer.add({
          tag: 'list-value',
          val: (value as unknown[]).map((v) => requiredIndex(item.write(v, writer), 'list item')),
        }),
      read: (reader, index) =>
        reader.node(index, 'list-value', (node) =>
          (node as Extract<WireValueNode, { tag: 'list-value' }>).val.map((i) =>
            item.read(reader, i),
          ),
        ),
    };
  }
  if (codec.optionInner) {
    const inner = child(codec.optionInner);
    if (!inner) return undefined;
    const none = codec.optionKind === 'nullable' ? null : undefined;
    return {
      write: (value, writer) =>
        writer.add({
          tag: 'option-value',
          val:
            value === none
              ? undefined
              : requiredIndex(inner.write(value, writer), 'option payload'),
        }),
      read: (reader, index) =>
        reader.node(index, 'option-value', (node) => {
          const childIndex = (node as Extract<WireValueNode, { tag: 'option-value' }>).val;
          return childIndex === undefined ? none : inner.read(reader, childIndex);
        }),
    };
  }
  if (codec.resultOk && codec.resultErr) {
    const ok = child(codec.resultOk);
    const err = child(codec.resultErr);
    if (!ok || !err) return undefined;
    return {
      write: (value, writer) => {
        const result = value as { tag: 'ok' | 'err'; val: unknown };
        if (result.tag !== 'ok' && result.tag !== 'err') {
          throw new TypeError('result value must have an ok or err tag');
        }
        const val =
          result.tag === 'ok' ? ok.write(result.val, writer) : err.write(result.val, writer);
        return writer.add({
          tag: 'result-value',
          val: { tag: result.tag === 'ok' ? 'ok-value' : 'err-value', val },
        });
      },
      read: (reader, index) =>
        reader.node(index, 'result-value', (node) => {
          const result = (node as Extract<WireValueNode, { tag: 'result-value' }>).val;
          return result.tag === 'ok-value'
            ? Result.ok(ok.read(reader, result.val))
            : Result.err(err.read(reader, result.val));
        }),
    };
  }
  const primitive = new Map<SchemaType['body']['tag'], WireValueNode['tag']>([
    ['bool', 'bool-value'],
    ['s8', 's8-value'],
    ['s16', 's16-value'],
    ['s32', 's32-value'],
    ['s64', 's64-value'],
    ['u8', 'u8-value'],
    ['u16', 'u16-value'],
    ['u32', 'u32-value'],
    ['u64', 'u64-value'],
    ['f32', 'f32-value'],
    ['f64', 'f64-value'],
    ['char', 'char-value'],
    ['string', 'string-value'],
  ]).get(codec.graph.root.body.tag);
  if (!primitive) return undefined;
  const body = codec.graph.root.body;
  const tag = body.tag;
  const restrictions = 'restrictions' in body ? body.restrictions : undefined;
  const integerBits = /^(s|u)(8|16|32|64)$/.exec(tag);
  const min = integerBits?.[1] === 's' ? -(2n ** (BigInt(integerBits[2]) - 1n)) : 0n;
  const max = integerBits
    ? 2n ** (BigInt(integerBits[2]) - (integerBits[1] === 's' ? 1n : 0n)) - 1n
    : 0n;
  const checked = (value: unknown): unknown => {
    let valid: boolean;
    if (integerBits) {
      valid =
        integerBits[2] === '64'
          ? typeof value === 'bigint'
          : typeof value === 'number' && Number.isInteger(value);
      if (valid) {
        const integer = BigInt(value as number | bigint);
        valid =
          integer >= min &&
          integer <= max &&
          numericRestrictionsMatch(
            restrictions as Parameters<typeof numericRestrictionsMatch>[0],
            integer,
          );
      }
    } else if (tag === 'f32' || tag === 'f64') {
      valid =
        typeof value === 'number' &&
        numericRestrictionsMatch(
          restrictions as Parameters<typeof numericRestrictionsMatch>[0],
          tag === 'f32' ? Math.fround(value) : value,
        );
    } else if (tag === 'bool') {
      valid = typeof value === 'boolean';
    } else {
      valid = typeof value === 'string';
      if (valid && tag === 'char') {
        const points = [...(value as string)];
        const code = points[0]?.codePointAt(0);
        valid = points.length === 1 && code !== undefined && (code < 0xd800 || code > 0xdfff);
      }
    }
    if (!valid) throw new TypeError('does not match its declared schema');
    return tag === 'f32' ? Math.fround(value as number) : value;
  };
  return {
    write: (value, writer) => writer.add({ tag: primitive, val: checked(value) } as WireValueNode),
    read: (reader, index) =>
      reader.node(index, primitive, (node) =>
        checked((node as WireValueNode & { val: unknown }).val),
      ),
  };
}

function requiredIndex(index: number | undefined, position: string): number {
  if (index === undefined) throw new TypeError(`${position} cannot be unit`);
  return index;
}

/** Recursively freeze codec data once compilation is complete. */
export function freezeSchemaCodec(codec: SchemaCodec): SchemaCodec {
  freezeCodec(codec, new WeakSet(), new WeakSet());
  return codec;
}

function freezeCodec(
  codec: SchemaCodec,
  seenCodecs: WeakSet<object>,
  seenGraphValues: WeakSet<object>,
): void {
  if (seenCodecs.has(codec)) return;
  seenCodecs.add(codec);

  freezeSchemaValue(codec.graph, seenGraphValues);
  if (codec.fields) {
    codec.fields.forEach((entry) => {
      freezeCodec(entry.codec, seenCodecs, seenGraphValues);
      Object.freeze(entry);
    });
    Object.freeze(codec.fields);
  }
  [codec.optionInner, codec.listItem, codec.mapKey, codec.mapValue, codec.secretInner].forEach(
    (child) => {
      if (child) freezeCodec(child, seenCodecs, seenGraphValues);
    },
  );
  if (codec.concrete?.tag === 'variant' || codec.concrete?.tag === 'multimodal') {
    codec.concrete.cases.forEach((entry) => {
      if (entry.codec) freezeCodec(entry.codec, seenCodecs, seenGraphValues);
      Object.freeze(entry);
    });
    Object.freeze(codec.concrete.cases);
  }
  if (codec.concrete) Object.freeze(codec.concrete);
  Object.freeze(codec);
}

/**
 * A per-vendor schema walker. Given a schema (a Standard Schema value of a known
 * vendor) and a `recurse` callback for child schemas, it produces a `SchemaCodec`.
 * Only the walker is vendor-specific; `SchemaCodec` is vendor-neutral.
 */
export type SchemaWalker = (
  schema: unknown,
  recurse: (child: unknown) => SchemaCodec,
) => SchemaCodec;

/**
 * Check that encoding and decoding `source` preserves its source shape without
 * consuming any affine handle retained in `encoded` for the real wire transfer.
 */
export function sourceValueIsCanonical(
  codec: SchemaCodec,
  source: unknown,
  encoded: SchemaValue,
): boolean {
  if (!graphMayContainCapability(codec.graph)) {
    return deepEqual(codec.fromValue(encoded), source);
  }

  const sentinels = new Map<unknown, object>();
  const probe = cloneWithSentinelHandles(encoded, sentinels);
  try {
    return deepEqual(source, codec.fromValue(probe), (raw, sentinel) => {
      return sentinels.has(raw) && sentinels.get(raw) === sentinel;
    });
  } finally {
    drainCapabilityHandles(probe);
  }
}

/**
 * Check a decoded schema value for codec-specific canonicality without moving
 * any affine handle that will subsequently be delivered to application code.
 */
export function schemaValueIsCanonical(codec: SchemaCodec, value: SchemaValue): boolean {
  if (!graphMayContainCapability(codec.graph)) {
    return deepEqual(codec.toValue(codec.fromValue(value)), value);
  }

  const sentinels = new Map<unknown, object>();
  const expected = cloneWithSentinelHandles(value, sentinels);
  const probe = cloneWithSentinelHandles(value, sentinels);
  let roundTrip: SchemaValue | undefined;
  try {
    roundTrip = codec.toValue(codec.fromValue(probe));
    return deepEqual(schemaValueToWit(roundTrip), schemaValueToWit(expected));
  } finally {
    drainCapabilityHandles(probe);
    drainCapabilityHandles(expected);
    if (roundTrip !== undefined) drainCapabilityHandles(roundTrip);
  }
}

const capabilityGraphCache = new WeakMap<SchemaGraph, boolean>();

function graphMayContainCapability(graph: SchemaGraph): boolean {
  const cached = capabilityGraphCache.get(graph);
  if (cached !== undefined) return cached;

  const visitedRefs = new Set<string>();
  const visit = (type: SchemaType): boolean => {
    const body = type.body;
    switch (body.tag) {
      case 'secret':
      case 'quota-token':
      case 'permission-card':
        return true;
      case 'ref': {
        if (visitedRefs.has(body.id)) return false;
        visitedRefs.add(body.id);
        const definition = graph.defs.get(body.id);
        return definition !== undefined && visit(definition.body);
      }
      case 'record':
        return body.fields.some((field) => visit(field.body));
      case 'variant':
        return body.cases.some(
          (variant) => variant.payload !== undefined && visit(variant.payload),
        );
      case 'tuple':
        return body.elements.some(visit);
      case 'list':
      case 'fixed-list':
      case 'option':
        return visit(body.element);
      case 'map':
        return visit(body.key) || visit(body.value);
      case 'result':
        return (
          (body.ok !== undefined && visit(body.ok)) || (body.err !== undefined && visit(body.err))
        );
      case 'union':
        return body.branches.some((branch) => visit(branch.body));
      case 'future':
      case 'stream':
        return body.element !== undefined && visit(body.element);
      default:
        return false;
    }
  };

  const result = visit(graph.root);
  capabilityGraphCache.set(graph, result);
  return result;
}

function cloneWithSentinelHandles(
  value: SchemaValue,
  sentinels: Map<unknown, object>,
): SchemaValue {
  const sentinelFor = (raw: unknown): object => {
    const existing = sentinels.get(raw);
    if (existing !== undefined) return existing;
    const sentinel = Object.freeze({});
    sentinels.set(raw, sentinel);
    return sentinel;
  };

  switch (value.tag) {
    case 'secret': {
      const raw = peekGuestSecretHandle(SECRET_INTERNAL, value.handle);
      if (raw === undefined) throw new Error('secret handle was already transferred');
      return {
        tag: 'secret',
        handle: createUntrackedGuestSecretHandle(SECRET_INTERNAL, sentinelFor(raw) as RawSecret),
      };
    }
    case 'quota-token': {
      const raw = peekGuestQuotaTokenHandle(QUOTA_INTERNAL, value.handle);
      if (raw === undefined) throw new Error('quota-token handle was already transferred');
      return {
        tag: 'quota-token',
        handle: createUntrackedGuestQuotaTokenHandle(
          QUOTA_INTERNAL,
          sentinelFor(raw) as RawQuotaToken,
        ),
      };
    }
    case 'permission-card': {
      const raw = peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, value.handle);
      if (raw === undefined) throw new Error('permission-card handle was already transferred');
      return {
        tag: 'permission-card',
        handle: createUntrackedGuestPermissionCardHandle(
          PERMISSION_CARD_INTERNAL,
          sentinelFor(raw) as RawPermissionCard,
        ),
      };
    }
    case 'record':
      return {
        tag: 'record',
        fields: value.fields.map((field) => cloneWithSentinelHandles(field, sentinels)),
      };
    case 'variant':
      return {
        tag: 'variant',
        caseIndex: value.caseIndex,
        payload:
          value.payload === undefined
            ? undefined
            : cloneWithSentinelHandles(value.payload, sentinels),
      };
    case 'tuple':
      return {
        tag: 'tuple',
        elements: value.elements.map((element) => cloneWithSentinelHandles(element, sentinels)),
      };
    case 'list':
      return {
        tag: 'list',
        elements: value.elements.map((element) => cloneWithSentinelHandles(element, sentinels)),
      };
    case 'fixed-list':
      return {
        tag: 'fixed-list',
        elements: value.elements.map((element) => cloneWithSentinelHandles(element, sentinels)),
      };
    case 'map':
      return {
        tag: 'map',
        entries: value.entries.map((entry) => ({
          key: cloneWithSentinelHandles(entry.key, sentinels),
          value: cloneWithSentinelHandles(entry.value, sentinels),
        })),
      };
    case 'option':
      return {
        tag: 'option',
        value:
          value.value === undefined ? undefined : cloneWithSentinelHandles(value.value, sentinels),
      };
    case 'result':
      return {
        tag: 'result',
        result: {
          tag: value.result.tag,
          value:
            value.result.value === undefined
              ? undefined
              : cloneWithSentinelHandles(value.result.value, sentinels),
        },
      };
    case 'union':
      return {
        tag: 'union',
        unionTag: value.unionTag,
        body: cloneWithSentinelHandles(value.body, sentinels),
      };
    default:
      return cloneSchemaValue(value);
  }
}

function drainCapabilityHandles(value: SchemaValue): void {
  switch (value.tag) {
    case 'secret':
      takeGuestSecretHandle(SECRET_INTERNAL, value.handle);
      return;
    case 'quota-token':
      takeGuestQuotaTokenHandle(QUOTA_INTERNAL, value.handle);
      return;
    case 'permission-card':
      takeGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, value.handle);
      return;
    case 'record':
      value.fields.forEach(drainCapabilityHandles);
      return;
    case 'variant':
      if (value.payload !== undefined) drainCapabilityHandles(value.payload);
      return;
    case 'tuple':
    case 'list':
    case 'fixed-list':
      value.elements.forEach(drainCapabilityHandles);
      return;
    case 'map':
      value.entries.forEach((entry) => {
        drainCapabilityHandles(entry.key);
        drainCapabilityHandles(entry.value);
      });
      return;
    case 'option':
      if (value.value !== undefined) drainCapabilityHandles(value.value);
      return;
    case 'result':
      if (value.result.value !== undefined) drainCapabilityHandles(value.result.value);
      return;
    case 'union':
      drainCapabilityHandles(value.body);
      return;
    default:
      return;
  }
}

export function relinquishSchemaValueCapabilities(value: SchemaValue): void {
  switch (value.tag) {
    case 'secret':
      releaseGuestSecretHandle(SECRET_INTERNAL, value.handle);
      return;
    case 'quota-token':
      releaseGuestQuotaTokenHandle(QUOTA_INTERNAL, value.handle);
      return;
    case 'permission-card':
      releaseGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, value.handle);
      return;
    case 'record':
      value.fields.forEach(relinquishSchemaValueCapabilities);
      return;
    case 'variant':
      if (value.payload !== undefined) relinquishSchemaValueCapabilities(value.payload);
      return;
    case 'tuple':
    case 'list':
    case 'fixed-list':
      value.elements.forEach(relinquishSchemaValueCapabilities);
      return;
    case 'map':
      value.entries.forEach((entry) => {
        relinquishSchemaValueCapabilities(entry.key);
        relinquishSchemaValueCapabilities(entry.value);
      });
      return;
    case 'option':
      if (value.value !== undefined) relinquishSchemaValueCapabilities(value.value);
      return;
    case 'result':
      if (value.result.value !== undefined) relinquishSchemaValueCapabilities(value.result.value);
      return;
    case 'union':
      relinquishSchemaValueCapabilities(value.body);
      return;
    default:
      return;
  }
}
