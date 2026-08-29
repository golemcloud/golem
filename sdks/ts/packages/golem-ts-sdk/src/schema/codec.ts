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
} from '../internal/schema-model';
import { GuestSecretHandle } from '../internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../internal/schema-model/secretInternal';
import { GuestQuotaTokenHandle } from '../internal/schema-model/quotaTokenHandle';
import { QUOTA_INTERNAL } from '../internal/schema-model/quotaInternal';
import { GuestPermissionCardHandle } from '../internal/schema-model/permissionCardHandle';
import { PERMISSION_CARD_INTERNAL } from '../internal/schema-model/permissionCardInternal';
import type {
  PermissionCard as RawPermissionCard,
  QuotaToken as RawQuotaToken,
  Secret as RawSecret,
} from 'golem:core/types@2.0.0';
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
  /** Child codecs for a WIT `map`, when the source schema exposes them. */
  readonly mapKey?: SchemaCodec;
  readonly mapValue?: SchemaCodec;
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

  freezeGraphValue(codec.graph, seenGraphValues);
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
  Object.freeze(codec);
}

function freezeGraphValue(value: unknown, seen: WeakSet<object>): void {
  if (value === null || (typeof value !== 'object' && typeof value !== 'function')) return;
  if (seen.has(value)) return;
  seen.add(value);

  if (value instanceof Map) {
    value.forEach((entryValue, key) => {
      freezeGraphValue(key, seen);
      freezeGraphValue(entryValue, seen);
    });
    Object.defineProperties(value, {
      set: { value: immutableMapMutation },
      delete: { value: immutableMapMutation },
      clear: { value: immutableMapMutation },
    });
    Object.freeze(value);
    return;
  }

  Reflect.ownKeys(value).forEach((key) => {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (descriptor && 'value' in descriptor) freezeGraphValue(descriptor.value, seen);
  });
  Object.freeze(value);
}

function immutableMapMutation(): never {
  throw new TypeError('Cannot mutate an immutable codec map');
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

  const probe = codec.toValue(source);
  try {
    return deepEqual(codec.fromValue(probe), source);
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
      const raw = value.handle.withHandle((handle) => handle);
      if (raw === undefined) throw new Error('secret handle was already transferred');
      return {
        tag: 'secret',
        handle: GuestSecretHandle.fromRaw(SECRET_INTERNAL, sentinelFor(raw) as RawSecret),
      };
    }
    case 'quota-token': {
      const raw = value.handle.withHandle((handle) => handle);
      if (raw === undefined) throw new Error('quota-token handle was already transferred');
      return {
        tag: 'quota-token',
        handle: GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, sentinelFor(raw) as RawQuotaToken),
      };
    }
    case 'permission-card': {
      const raw = value.handle.withHandle((handle) => handle);
      if (raw === undefined) throw new Error('permission-card handle was already transferred');
      return {
        tag: 'permission-card',
        handle: GuestPermissionCardHandle.fromRaw(
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
    case 'quota-token':
    case 'permission-card':
      value.handle.take();
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
