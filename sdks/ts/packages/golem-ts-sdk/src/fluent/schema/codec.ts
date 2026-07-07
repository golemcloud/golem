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

export interface FluentCodec {
  /** Root SchemaType and the nominal definitions it references. */
  readonly graph: SchemaGraph;
  toValue(value: unknown): SchemaValue;
  fromValue(value: SchemaValue): unknown;
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

/**
 * A per-vendor schema walker. Given a schema (a Standard Schema value of a known
 * vendor) and a `recurse` callback for child schemas, it produces a `FluentCodec`.
 * Only the walker is vendor-specific; `FluentCodec` is vendor-neutral.
 */
export type SchemaWalker = (
  schema: unknown,
  recurse: (child: unknown) => FluentCodec,
) => FluentCodec;
