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

// Recursive, in-memory mirror of the `golem:core/types@2.0.0` schema model.
//
// The WIT carrier (`schema-graph` / `schema-value-tree`) is flat-with-indices
// because WIT has no recursive types. This module defines the ergonomic
// recursive forms (`SchemaType`, `SchemaValue`, `SchemaGraph`,
// `TypedSchemaValue`) that the rest of the SDK works with; `./wit.ts` converts
// to and from the flat carrier mechanically.
//
// Non-recursive leaf structures (restrictions, specs, metadata, capability
// payloads, datetime, …) are reused directly from the generated bindings so
// field names and shapes match the wire format exactly and the codecs can pass
// them through unchanged.

import type {
  TypeId,
  MetadataEnvelope,
  TextRestrictions,
  BinaryRestrictions,
  PathSpec,
  UrlRestrictions,
  QuantitySpec,
  QuantityValue,
  SecretSpec,
  QuotaTokenSpec,
  DiscriminatorRule,
  Datetime,
  Uuid,
  EnvironmentId,
} from 'golem:core/types@2.0.0';
import { GuestQuotaTokenHandle } from './quotaTokenHandle';

export type {
  TypeId,
  MetadataEnvelope,
  TextRestrictions,
  BinaryRestrictions,
  PathSpec,
  UrlRestrictions,
  QuantitySpec,
  QuantityValue,
  SecretSpec,
  QuotaTokenSpec,
  DiscriminatorRule,
  Datetime,
  Uuid,
  EnvironmentId,
};

// These are part of the schema-model public surface but are only ever re-exported
// (never referenced in a local declaration), so re-export them directly to avoid
// an "unused external import" bundling warning.
export type { Role, PathDirection, PathKind, FieldDiscriminator } from 'golem:core/types@2.0.0';

// ============================================================
// Schema type (recursive)
// ============================================================

/** A schema type node: a structural body plus its metadata envelope. */
export interface SchemaType {
  body: SchemaTypeBody;
  metadata: MetadataEnvelope;
}

export type SchemaTypeBody =
  // Reference to a named definition in the enclosing graph.
  | { tag: 'ref'; id: TypeId }
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
  // Structural composites
  | { tag: 'record'; fields: NamedFieldType[] }
  | { tag: 'variant'; cases: VariantCaseType[] }
  | { tag: 'enum'; cases: string[] }
  | { tag: 'flags'; names: string[] }
  | { tag: 'tuple'; elements: SchemaType[] }
  | { tag: 'list'; element: SchemaType }
  | { tag: 'fixed-list'; element: SchemaType; length: number }
  | { tag: 'map'; key: SchemaType; value: SchemaType }
  | { tag: 'option'; element: SchemaType }
  | { tag: 'result'; ok?: SchemaType; err?: SchemaType }
  // Rich semantic types
  | { tag: 'text'; restrictions: TextRestrictions }
  | { tag: 'binary'; restrictions: BinaryRestrictions }
  | { tag: 'path'; spec: PathSpec }
  | { tag: 'url'; restrictions: UrlRestrictions }
  | { tag: 'datetime' }
  | { tag: 'duration' }
  | { tag: 'quantity'; spec: QuantitySpec }
  // Discriminated union (closed, inferred-tag)
  | { tag: 'union'; branches: UnionBranch[] }
  // Capability nodes
  | { tag: 'secret'; spec: SecretSpec }
  | { tag: 'quota-token'; spec: QuotaTokenSpec }
  // WASI P3 stubs (parseable only; no semantics yet)
  | { tag: 'future'; element?: SchemaType }
  | { tag: 'stream'; element?: SchemaType };

export interface NamedFieldType {
  name: string;
  body: SchemaType;
  metadata: MetadataEnvelope;
}

export interface VariantCaseType {
  name: string;
  payload?: SchemaType;
  metadata: MetadataEnvelope;
}

export interface UnionBranch {
  /** Logical branch name (carried in `union` values). */
  tag: string;
  body: SchemaType;
  discriminator: DiscriminatorRule;
  metadata: MetadataEnvelope;
}

// ============================================================
// Schema graph (recursive, self-contained)
// ============================================================

export interface SchemaTypeDef {
  /** Optional human-readable qualified name (display only). */
  name?: string;
  body: SchemaType;
}

/**
 * A self-contained schema graph: a registry of named definitions (keyed by
 * stable `type-id`) plus a root type. `ref` bodies reference entries in `defs`.
 */
export interface SchemaGraph {
  defs: Map<TypeId, SchemaTypeDef>;
  root: SchemaType;
}

// ============================================================
// Schema value (recursive)
// ============================================================

export type SchemaValue =
  // Primitives
  | { tag: 'bool'; value: boolean }
  | { tag: 's8'; value: number }
  | { tag: 's16'; value: number }
  | { tag: 's32'; value: number }
  | { tag: 's64'; value: bigint }
  | { tag: 'u8'; value: number }
  | { tag: 'u16'; value: number }
  | { tag: 'u32'; value: number }
  | { tag: 'u64'; value: bigint }
  | { tag: 'f32'; value: number }
  | { tag: 'f64'; value: number }
  | { tag: 'char'; value: string }
  | { tag: 'string'; value: string }
  // Structural composites
  | { tag: 'record'; fields: SchemaValue[] }
  | { tag: 'variant'; caseIndex: number; payload?: SchemaValue }
  | { tag: 'enum'; caseIndex: number }
  | { tag: 'flags'; flags: boolean[] }
  | { tag: 'tuple'; elements: SchemaValue[] }
  | { tag: 'list'; elements: SchemaValue[] }
  | { tag: 'fixed-list'; elements: SchemaValue[] }
  | { tag: 'map'; entries: SchemaMapEntry[] }
  | { tag: 'option'; value?: SchemaValue }
  | { tag: 'result'; result: SchemaResult }
  // Rich semantic
  | { tag: 'text'; text: string; language?: string }
  | { tag: 'binary'; bytes: Uint8Array; mimeType?: string }
  | { tag: 'path'; value: string }
  | { tag: 'url'; value: string }
  | { tag: 'datetime'; value: Datetime }
  | { tag: 'duration'; nanoseconds: bigint }
  | { tag: 'quantity'; value: QuantityValue }
  // Discriminated union
  | { tag: 'union'; unionTag: string; body: SchemaValue }
  // Capability nodes
  | { tag: 'secret'; secretRef: string }
  // An opaque, affine owned `quota-token` handle. Carried by ownership; never
  // inspectable or forgeable from a guest. See `GuestQuotaTokenHandle`.
  | { tag: 'quota-token'; handle: GuestQuotaTokenHandle };

export interface SchemaMapEntry {
  key: SchemaValue;
  value: SchemaValue;
}

export type SchemaResult = { tag: 'ok'; value?: SchemaValue } | { tag: 'err'; value?: SchemaValue };

// ============================================================
// Wire carrier (recursive)
// ============================================================

export interface TypedSchemaValue {
  graph: SchemaGraph;
  value: SchemaValue;
}

// ============================================================
// Constructors / helpers
// ============================================================

export function emptyMetadata(): MetadataEnvelope {
  return { aliases: [], examples: [] };
}

/** Wrap a body into a `SchemaType` node with empty metadata. */
export function schemaType(
  body: SchemaTypeBody,
  metadata: MetadataEnvelope = emptyMetadata(),
): SchemaType {
  return { body, metadata };
}

/** Compact constructors for schema types (anonymous unless wrapped in a def). */
export const t = {
  ref: (id: TypeId): SchemaType => schemaType({ tag: 'ref', id }),
  bool: (): SchemaType => schemaType({ tag: 'bool' }),
  s8: (): SchemaType => schemaType({ tag: 's8' }),
  s16: (): SchemaType => schemaType({ tag: 's16' }),
  s32: (): SchemaType => schemaType({ tag: 's32' }),
  s64: (): SchemaType => schemaType({ tag: 's64' }),
  u8: (): SchemaType => schemaType({ tag: 'u8' }),
  u16: (): SchemaType => schemaType({ tag: 'u16' }),
  u32: (): SchemaType => schemaType({ tag: 'u32' }),
  u64: (): SchemaType => schemaType({ tag: 'u64' }),
  f32: (): SchemaType => schemaType({ tag: 'f32' }),
  f64: (): SchemaType => schemaType({ tag: 'f64' }),
  char: (): SchemaType => schemaType({ tag: 'char' }),
  string: (): SchemaType => schemaType({ tag: 'string' }),
  record: (fields: NamedFieldType[]): SchemaType => schemaType({ tag: 'record', fields }),
  variant: (cases: VariantCaseType[]): SchemaType => schemaType({ tag: 'variant', cases }),
  enum: (cases: string[]): SchemaType => schemaType({ tag: 'enum', cases }),
  flags: (names: string[]): SchemaType => schemaType({ tag: 'flags', names }),
  tuple: (elements: SchemaType[]): SchemaType => schemaType({ tag: 'tuple', elements }),
  list: (element: SchemaType): SchemaType => schemaType({ tag: 'list', element }),
  fixedList: (element: SchemaType, length: number): SchemaType =>
    schemaType({ tag: 'fixed-list', element, length }),
  map: (key: SchemaType, value: SchemaType): SchemaType => schemaType({ tag: 'map', key, value }),
  option: (element: SchemaType): SchemaType => schemaType({ tag: 'option', element }),
  result: (ok?: SchemaType, err?: SchemaType): SchemaType => schemaType({ tag: 'result', ok, err }),
  datetime: (): SchemaType => schemaType({ tag: 'datetime' }),
  duration: (): SchemaType => schemaType({ tag: 'duration' }),
  quotaToken: (spec: QuotaTokenSpec): SchemaType => schemaType({ tag: 'quota-token', spec }),
};

/** Compact constructors for schema field/case helpers. */
export function field(
  name: string,
  body: SchemaType,
  metadata: MetadataEnvelope = emptyMetadata(),
): NamedFieldType {
  return { name, body, metadata };
}

export function variantCase(
  name: string,
  payload?: SchemaType,
  metadata: MetadataEnvelope = emptyMetadata(),
): VariantCaseType {
  return { name, payload, metadata };
}

/** Compact constructors for schema values. */
export const v = {
  bool: (value: boolean): SchemaValue => ({ tag: 'bool', value }),
  s8: (value: number): SchemaValue => ({ tag: 's8', value }),
  s16: (value: number): SchemaValue => ({ tag: 's16', value }),
  s32: (value: number): SchemaValue => ({ tag: 's32', value }),
  s64: (value: bigint): SchemaValue => ({ tag: 's64', value }),
  u8: (value: number): SchemaValue => ({ tag: 'u8', value }),
  u16: (value: number): SchemaValue => ({ tag: 'u16', value }),
  u32: (value: number): SchemaValue => ({ tag: 'u32', value }),
  u64: (value: bigint): SchemaValue => ({ tag: 'u64', value }),
  f32: (value: number): SchemaValue => ({ tag: 'f32', value }),
  f64: (value: number): SchemaValue => ({ tag: 'f64', value }),
  char: (value: string): SchemaValue => ({ tag: 'char', value }),
  string: (value: string): SchemaValue => ({ tag: 'string', value }),
  record: (fields: SchemaValue[]): SchemaValue => ({ tag: 'record', fields }),
  variant: (caseIndex: number, payload?: SchemaValue): SchemaValue => ({
    tag: 'variant',
    caseIndex,
    payload,
  }),
  enum: (caseIndex: number): SchemaValue => ({ tag: 'enum', caseIndex }),
  flags: (flags: boolean[]): SchemaValue => ({ tag: 'flags', flags }),
  tuple: (elements: SchemaValue[]): SchemaValue => ({ tag: 'tuple', elements }),
  list: (elements: SchemaValue[]): SchemaValue => ({ tag: 'list', elements }),
  fixedList: (elements: SchemaValue[]): SchemaValue => ({ tag: 'fixed-list', elements }),
  map: (entries: SchemaMapEntry[]): SchemaValue => ({ tag: 'map', entries }),
  option: (value?: SchemaValue): SchemaValue => ({ tag: 'option', value }),
  ok: (value?: SchemaValue): SchemaValue => ({ tag: 'result', result: { tag: 'ok', value } }),
  err: (value?: SchemaValue): SchemaValue => ({ tag: 'result', result: { tag: 'err', value } }),
  quotaToken: (handle: GuestQuotaTokenHandle): SchemaValue => ({ tag: 'quota-token', handle }),
};

// ============================================================
// Structural equality (bigint / Uint8Array / Map aware)
// ============================================================

/**
 * Deep structural equality that understands the value shapes used by the schema
 * model: bigint, Uint8Array, Map, arrays, and plain objects. Object keys whose
 * value is `undefined` are ignored, matching the WIT option lifting convention
 * (and Vitest's `toEqual`).
 */
export function deepEqual(a: unknown, b: unknown): boolean {
  // Numbers use `Object.is` so that `NaN` equals `NaN` and, crucially, `-0` does
  // NOT equal `0` (a real f32/f64 round-trip difference we must not mask).
  if (typeof a === 'number' && typeof b === 'number') {
    return Object.is(a, b);
  }

  if (a === b) return true;

  // Quota-token handles are affine capabilities, not structural data: equality
  // is identity only (mirrors the Rust shared-cell `PartialEq`). Without this,
  // two distinct handles would compare equal (both expose no enumerable state).
  if (a instanceof GuestQuotaTokenHandle || b instanceof GuestQuotaTokenHandle) return false;

  if (typeof a === 'bigint' || typeof b === 'bigint') return a === b;

  if (a === null || b === null || typeof a !== 'object' || typeof b !== 'object') {
    return false;
  }

  if (a instanceof Uint8Array || b instanceof Uint8Array) {
    if (!(a instanceof Uint8Array) || !(b instanceof Uint8Array)) return false;
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  }

  if (a instanceof Map || b instanceof Map) {
    if (!(a instanceof Map) || !(b instanceof Map)) return false;
    if (a.size !== b.size) return false;
    for (const [k, av] of a) {
      if (!b.has(k)) return false;
      if (!deepEqual(av, b.get(k))) return false;
    }
    return true;
  }

  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) return false;
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!deepEqual(a[i], b[i])) return false;
    return true;
  }

  const ao = a as Record<string, unknown>;
  const bo = b as Record<string, unknown>;
  const keys = new Set<string>();
  for (const k of Object.keys(ao)) if (ao[k] !== undefined) keys.add(k);
  for (const k of Object.keys(bo)) if (bo[k] !== undefined) keys.add(k);
  for (const k of keys) {
    if (!deepEqual(ao[k], bo[k])) return false;
  }
  return true;
}

/** Structural equality for two schema types. */
export function schemaTypeEquals(a: SchemaType, b: SchemaType): boolean {
  return deepEqual(a, b);
}

/** Structural equality for two schema values. */
export function schemaValueEquals(a: SchemaValue, b: SchemaValue): boolean {
  return deepEqual(a, b);
}
