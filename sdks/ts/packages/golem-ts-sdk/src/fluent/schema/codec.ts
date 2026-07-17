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

// `FluentCodec` pairs a schema's WIT type with its value codec: walking a schema
// once yields BOTH the WIT type (`SchemaGraph`) and the bidirectional value codec
// (`toValue`/`fromValue`). It depends only on the new schema model
// (`internal/schema-model/`), never on the decorator-era `Type.Type` resolvers.

import { SchemaGraph, SchemaValue } from '../../internal/schema-model';
import type { StandardSchemaV1 } from './standardSchema';

/**
 * A stable schema graph paired with deterministic value conversions. Codecs are
 * immutable once compiled: conversions must not mutate the codec, its graph, or
 * any codec reachable through structural child links.
 */
export interface FluentCodec {
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
  readonly fields?: ReadonlyArray<{ readonly name: string; readonly codec: FluentCodec }>;
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
  readonly optionInner?: FluentCodec;
  /** Item codec for a WIT `list` or `fixed-list`. */
  readonly listItem?: FluentCodec;
  /** Child codecs for a WIT `map`, when the source schema exposes them. */
  readonly mapKey?: FluentCodec;
  readonly mapValue?: FluentCodec;
  /**
   * For SECRET markers (`s.secret(inner)`): the inner (revealed-value) codec —
   * the one that decodes the plaintext after `golem:secrets/reveal`. The
   * marker's own `graph` is `secret<inner>` and its own `fromValue` yields the
   * raw handle; the config surface uses this inner codec to decode a revealed
   * secret leaf.
   */
  readonly secretInner?: FluentCodec;
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
export function freezeFluentCodec(codec: FluentCodec): FluentCodec {
  freezeCodec(codec, new WeakSet(), new WeakSet());
  return codec;
}

function freezeCodec(
  codec: FluentCodec,
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
 * vendor) and a `recurse` callback for child schemas, it produces a `FluentCodec`.
 * Only the walker is vendor-specific; `FluentCodec` is vendor-neutral.
 */
export type SchemaWalker = (
  schema: unknown,
  recurse: (child: unknown) => FluentCodec,
) => FluentCodec;
