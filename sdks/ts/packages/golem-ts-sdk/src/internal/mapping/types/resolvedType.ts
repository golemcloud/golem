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

// `ResolvedType` is the TypeScript SDK's in-memory type representation. It is
// structurally the new schema model (`golem:core/types@2.0.0` —
// `record`/`variant`/`enum`/`flags`/`tuple`/`list`/`map`/`option`/`result`/…)
// rather than the legacy analysed-type model. Because TypeScript is dynamically
// typed, a handful of *representation hints* — information the wire schema
// deliberately does NOT carry — are attached so that values can be reconstructed
// as idiomatic JS on the way back in:
//
//   - typed arrays (`Uint8Array` vs `number[]`)
//   - option `None` reconstructed as `null` vs `undefined`
//   - `Result<T, E>` represented as the inbuilt `{ tag, val }` vs a user-defined
//     record with named `ok`/`err` fields
//   - tagged-union case payload key names
//   - the empty/void tuple shape (`null`/`undefined`)
//
// These hints live ONLY in `ResolvedType`; the projection to the wire
// `SchemaType` (see `./schemaType.ts`) drops them, and the value codec (see
// `../values/schemaValue.ts`) consumes them when turning a `SchemaValue` back
// into a TS value.
//
// 64-bit integers always serialize/deserialize as `bigint` (matching the
// `SchemaValue` model); there is therefore no `number`-vs-`bigint` hint.
//
// Recursive (and mutually-recursive) types are represented WITHOUT object
// cycles: a recursive back-edge is a `ref` body carrying the stable `type-id`
// of the named composite it points at (exactly like `SchemaType.ref` and the
// reflection's `{ kind: 'others', recursive: true }` back-edge). The actual
// composite bodies live in a `ResolvedGraph.defs` registry keyed by that id.
// Keeping `ResolvedType` acyclic means structural hashing / `JSON.stringify`
// stay valid even for recursive graphs.

import type {
  PathSpec,
  QuantitySpec,
  QuotaTokenSpec,
  TypeId,
  UrlRestrictions,
} from '../../schema-model';

export type { TypeId };

/** Typed-array element kinds recognised by the SDK. */
export type TypedArrayKind =
  | 'u8'
  | 'u16'
  | 'u32'
  | 'big-u64'
  | 'i8'
  | 'i16'
  | 'i32'
  | 'big-i64'
  | 'f32'
  | 'f64';

/** How an absent value (`None` / empty / void) is represented in TS. */
export type AbsentRepr = 'null' | 'undefined';

/**
 * How a `Result<T, E>` is represented in TS:
 *   - `inbuilt`: the SDK `Result` shape `{ tag: 'ok' | 'err', val? }`; the
 *     `okAbsent` / `errAbsent` fields say how a missing payload is materialised.
 *   - `custom`: a user-defined tagged record `{ tag: 'ok' | 'err', [name]: … }`;
 *     `okValueName` / `errValueName` carry the payload field names.
 */
export type ResultRepr =
  | { tag: 'inbuilt'; okAbsent?: AbsentRepr; errAbsent?: AbsentRepr }
  | { tag: 'custom'; okValueName?: string; errValueName?: string };

/** A single named field of a `record`. */
export interface ResolvedField {
  name: string;
  type: ResolvedType;
}

/**
 * A single case of a `variant`.
 *   - `payload` is the case's payload type (absent for unit cases).
 *   - `valueKey`, when present, is the property name holding the payload in a
 *     tagged-union object (`{ tag, [valueKey]: payload }`). When absent on a
 *     case with a payload the variant is a *plain* union and the case wraps the
 *     payload value directly.
 */
export interface ResolvedVariantCase {
  name: string;
  payload?: ResolvedType;
  valueKey?: string;
}

export type ResolvedTypeBody =
  // Primitives
  | { tag: 'bool' }
  | { tag: 's8' }
  | { tag: 's16' }
  | { tag: 's32' }
  | { tag: 's64' }
  | { tag: 'u8' }
  | { tag: 'u16' }
  | { tag: 'u32' }
  | { tag: 'u64' }
  | { tag: 'f32' }
  | { tag: 'f64' }
  | { tag: 'char' }
  | { tag: 'string' }
  // Composites
  | { tag: 'list'; element: ResolvedType; typedArray?: TypedArrayKind }
  | { tag: 'map'; key: ResolvedType; value: ResolvedType }
  | { tag: 'tuple'; elements: ResolvedType[]; empty?: AbsentRepr }
  | { tag: 'record'; fields: ResolvedField[] }
  | { tag: 'variant'; tagged: boolean; cases: ResolvedVariantCase[] }
  | { tag: 'enum'; cases: string[] }
  | { tag: 'flags'; names: string[] }
  | { tag: 'option'; element: ResolvedType; noneRepr: AbsentRepr }
  | { tag: 'result'; ok?: ResolvedType; err?: ResolvedType; repr: ResultRepr }
  // The opaque `quota-token` capability. Carries only its spec; the runtime
  // value is an unforgeable owned handle (see `GuestQuotaTokenHandle`), never a
  // structural record.
  | { tag: 'quota-token'; spec: QuotaTokenSpec }
  | { tag: 'path'; spec: PathSpec }
  | { tag: 'url'; restrictions: UrlRestrictions }
  | { tag: 'datetime' }
  | { tag: 'duration' }
  | { tag: 'quantity'; spec: QuantitySpec }
  // A reference to a named composite registered in a `ResolvedGraph.defs` under
  // `id`. Used to close recursive / mutually-recursive cycles without object
  // cycles. Purely structural: occurrence-level hints (option / list / tuple /
  // result repr) are always carried by the wrapping node, never by the `ref`.
  | { tag: 'ref'; id: TypeId };

/**
 * A resolved type node: a structural body plus the optional nominal identity
 * (`name` / `owner`) used to register named/recursive types as graph
 * definitions when projecting to a `SchemaGraph`.
 */
export interface ResolvedType {
  body: ResolvedTypeBody;
  name?: string;
  owner?: string;
}

// ============================================================
// Constructors
// ============================================================

export function resolved(body: ResolvedTypeBody, name?: string, owner?: string): ResolvedType {
  return { body, name, owner };
}

export const r = {
  bool: (): ResolvedType => resolved({ tag: 'bool' }),
  s8: (): ResolvedType => resolved({ tag: 's8' }),
  s16: (): ResolvedType => resolved({ tag: 's16' }),
  s32: (): ResolvedType => resolved({ tag: 's32' }),
  s64: (): ResolvedType => resolved({ tag: 's64' }),
  u8: (): ResolvedType => resolved({ tag: 'u8' }),
  u16: (): ResolvedType => resolved({ tag: 'u16' }),
  u32: (): ResolvedType => resolved({ tag: 'u32' }),
  u64: (): ResolvedType => resolved({ tag: 'u64' }),
  f32: (): ResolvedType => resolved({ tag: 'f32' }),
  f64: (): ResolvedType => resolved({ tag: 'f64' }),
  char: (): ResolvedType => resolved({ tag: 'char' }),
  string: (): ResolvedType => resolved({ tag: 'string' }),
  list: (
    element: ResolvedType,
    typedArray?: TypedArrayKind,
    name?: string,
    owner?: string,
  ): ResolvedType => resolved({ tag: 'list', element, typedArray }, name, owner),
  map: (key: ResolvedType, value: ResolvedType, name?: string, owner?: string): ResolvedType =>
    resolved({ tag: 'map', key, value }, name, owner),
  tuple: (
    elements: ResolvedType[],
    empty?: AbsentRepr,
    name?: string,
    owner?: string,
  ): ResolvedType => resolved({ tag: 'tuple', elements, empty }, name, owner),
  record: (fields: ResolvedField[], name?: string, owner?: string): ResolvedType =>
    resolved({ tag: 'record', fields }, name, owner),
  variant: (
    tagged: boolean,
    cases: ResolvedVariantCase[],
    name?: string,
    owner?: string,
  ): ResolvedType => resolved({ tag: 'variant', tagged, cases }, name, owner),
  enum: (cases: string[], name?: string, owner?: string): ResolvedType =>
    resolved({ tag: 'enum', cases }, name, owner),
  flags: (names: string[], name?: string, owner?: string): ResolvedType =>
    resolved({ tag: 'flags', names }, name, owner),
  option: (
    element: ResolvedType,
    noneRepr: AbsentRepr,
    name?: string,
    owner?: string,
  ): ResolvedType => resolved({ tag: 'option', element, noneRepr }, name, owner),
  result: (
    ok: ResolvedType | undefined,
    err: ResolvedType | undefined,
    repr: ResultRepr,
    name?: string,
    owner?: string,
  ): ResolvedType => resolved({ tag: 'result', ok, err, repr }, name, owner),
  quotaToken: (spec: QuotaTokenSpec): ResolvedType => resolved({ tag: 'quota-token', spec }),
  path: (spec: PathSpec): ResolvedType => resolved({ tag: 'path', spec }),
  url: (restrictions: UrlRestrictions): ResolvedType => resolved({ tag: 'url', restrictions }),
  datetime: (): ResolvedType => resolved({ tag: 'datetime' }),
  duration: (): ResolvedType => resolved({ tag: 'duration' }),
  quantity: (spec: QuantitySpec): ResolvedType => resolved({ tag: 'quantity', spec }),
  ref: (id: TypeId): ResolvedType => resolved({ tag: 'ref', id }),
};

/**
 * A self-contained resolved type graph: a `root` type plus the registry of
 * named composite definitions (`record` / `variant` / `enum` / `flags`) it (and
 * recursive back-edges) reference by `ref` id. This is what the new
 * `ResolvedType`-native mapper produces and what the schema projection
 * (`resolvedGraphToSchemaType`) and the graph-aware value codec consume.
 */
export interface ResolvedGraph {
  defs: Map<TypeId, ResolvedType>;
  root: ResolvedType;
}

export function resolvedField(name: string, type: ResolvedType): ResolvedField {
  return { name, type };
}
