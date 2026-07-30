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
  NumericRestrictions,
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
import { GuestSecretHandle } from './secretHandle';
import { GuestQuotaTokenHandle } from './quotaTokenHandle';

export type {
  TypeId,
  MetadataEnvelope,
  NumericRestrictions,
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
export type {
  NumericBound,
  Role,
  PathDirection,
  PathKind,
  FieldDiscriminator,
} from 'golem:core/types@2.0.0';

// ============================================================
// Schema type (recursive)
// ============================================================

/** A schema type node: a structural body plus its metadata envelope. */
export interface SchemaType {
  readonly body: SchemaTypeBody;
  readonly metadata: MetadataEnvelope;
}

export type SchemaTypeBody =
  // Reference to a named definition in the enclosing graph.
  | { tag: 'ref'; id: TypeId }
  // Primitives
  | { tag: 'bool' }
  | { tag: 's8'; restrictions?: NumericRestrictions }
  | { tag: 's16'; restrictions?: NumericRestrictions }
  | { tag: 's32'; restrictions?: NumericRestrictions }
  | { tag: 's64'; restrictions?: NumericRestrictions }
  | { tag: 'u8'; restrictions?: NumericRestrictions }
  | { tag: 'u16'; restrictions?: NumericRestrictions }
  | { tag: 'u32'; restrictions?: NumericRestrictions }
  | { tag: 'u64'; restrictions?: NumericRestrictions }
  | { tag: 'f32'; restrictions?: NumericRestrictions }
  | { tag: 'f64'; restrictions?: NumericRestrictions }
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
  | { tag: 'secret'; spec: Omit<SecretSpec, 'inner'>; inner: SchemaType }
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
  readonly name?: string;
  readonly body: SchemaType;
}

/**
 * A self-contained schema graph: a registry of named definitions (keyed by
 * stable `type-id`) plus a root type. `ref` bodies reference entries in `defs`.
 */
export interface SchemaGraph {
  readonly defs: ReadonlyMap<TypeId, SchemaTypeDef>;
  readonly root: SchemaType;
}

const SCHEMA_SHAPE_MAX_DEPTH = 32;

/** Compare canonical value shape while ignoring metadata and refinable restrictions. */
export function schemaShapesMatch(left: SchemaGraph, right: SchemaGraph): boolean {
  return schemaTypesMatch(left, left.root, right, right.root, SCHEMA_SHAPE_MAX_DEPTH, new Map());
}

function schemaTypesMatch(
  leftGraph: SchemaGraph,
  leftType: SchemaType,
  rightGraph: SchemaGraph,
  rightType: SchemaType,
  depth: number,
  visiting: Map<TypeId, Set<TypeId>>,
): boolean {
  if (leftType.body.tag === 'ref' && rightType.body.tag === 'ref') {
    const rightIds = visiting.get(leftType.body.id);
    if (rightIds?.has(rightType.body.id)) return true;
    if (rightIds) rightIds.add(rightType.body.id);
    else visiting.set(leftType.body.id, new Set([rightType.body.id]));
  }

  const left = resolveShapeType(leftGraph, leftType);
  const right = resolveShapeType(rightGraph, rightType);
  if (!left || !right || depth === 0) return false;
  const next = depth - 1;

  if (left.tag === 'string' && right.tag === 'text') {
    return right.restrictions.languages === undefined;
  }
  if (left.tag === 'text' && right.tag === 'string') {
    return left.restrictions.languages === undefined;
  }
  if (left.tag !== right.tag) return false;

  switch (left.tag) {
    case 'record': {
      const other = right as Extract<SchemaTypeBody, { tag: 'record' }>;
      return (
        left.fields.length === other.fields.length &&
        left.fields.every(
          (field, index) =>
            field.name === other.fields[index].name &&
            schemaTypesMatch(
              leftGraph,
              field.body,
              rightGraph,
              other.fields[index].body,
              next,
              visiting,
            ),
        )
      );
    }
    case 'variant': {
      const other = right as Extract<SchemaTypeBody, { tag: 'variant' }>;
      return (
        left.cases.length === other.cases.length &&
        left.cases.every((variantCase, index) => {
          const otherCase = other.cases[index];
          return (
            variantCase.name === otherCase.name &&
            optionalSchemaTypesMatch(
              leftGraph,
              variantCase.payload,
              rightGraph,
              otherCase.payload,
              next,
              visiting,
            )
          );
        })
      );
    }
    case 'enum':
      return stringArraysEqual(left.cases, (right as typeof left).cases);
    case 'flags':
      return stringArraysEqual(left.names, (right as typeof left).names);
    case 'tuple': {
      const other = right as Extract<SchemaTypeBody, { tag: 'tuple' }>;
      return (
        left.elements.length === other.elements.length &&
        left.elements.every((element, index) =>
          schemaTypesMatch(leftGraph, element, rightGraph, other.elements[index], next, visiting),
        )
      );
    }
    case 'list':
    case 'option': {
      const other = right as typeof left;
      return schemaTypesMatch(leftGraph, left.element, rightGraph, other.element, next, visiting);
    }
    case 'fixed-list': {
      const other = right as Extract<SchemaTypeBody, { tag: 'fixed-list' }>;
      return (
        left.length === other.length &&
        schemaTypesMatch(leftGraph, left.element, rightGraph, other.element, next, visiting)
      );
    }
    case 'map': {
      const other = right as Extract<SchemaTypeBody, { tag: 'map' }>;
      return (
        schemaTypesMatch(leftGraph, left.key, rightGraph, other.key, next, visiting) &&
        schemaTypesMatch(leftGraph, left.value, rightGraph, other.value, next, visiting)
      );
    }
    case 'result': {
      const other = right as Extract<SchemaTypeBody, { tag: 'result' }>;
      return (
        optionalSchemaTypesMatch(leftGraph, left.ok, rightGraph, other.ok, next, visiting) &&
        optionalSchemaTypesMatch(leftGraph, left.err, rightGraph, other.err, next, visiting)
      );
    }
    case 'union': {
      const other = right as Extract<SchemaTypeBody, { tag: 'union' }>;
      return (
        left.branches.length === other.branches.length &&
        left.branches.every((branch, index) => {
          const otherBranch = other.branches[index];
          return (
            branch.tag === otherBranch.tag &&
            deepEqual(branch.discriminator, otherBranch.discriminator) &&
            schemaTypesMatch(leftGraph, branch.body, rightGraph, otherBranch.body, next, visiting)
          );
        })
      );
    }
    case 'quantity': {
      const other = right as Extract<SchemaTypeBody, { tag: 'quantity' }>;
      return (
        left.spec.baseUnit === other.spec.baseUnit &&
        stringSetsEqual(effectiveQuantityUnits(left.spec), effectiveQuantityUnits(other.spec))
      );
    }
    case 'secret': {
      const other = right as Extract<SchemaTypeBody, { tag: 'secret' }>;
      return (
        left.spec.category === other.spec.category &&
        schemaTypesMatch(leftGraph, left.inner, rightGraph, other.inner, next, visiting)
      );
    }
    case 'quota-token':
      return (
        left.spec.resourceName ===
        (right as Extract<SchemaTypeBody, { tag: 'quota-token' }>).spec.resourceName
      );
    case 'text':
      return optionalStringSetsEqual(
        left.restrictions.languages,
        (right as Extract<SchemaTypeBody, { tag: 'text' }>).restrictions.languages,
      );
    case 'binary':
      return optionalStringSetsEqual(
        left.restrictions.mimeTypes,
        (right as Extract<SchemaTypeBody, { tag: 'binary' }>).restrictions.mimeTypes,
      );
    case 'future':
    case 'stream': {
      const other = right as typeof left;
      return optionalSchemaTypesMatch(
        leftGraph,
        left.element,
        rightGraph,
        other.element,
        next,
        visiting,
      );
    }
    default:
      return true;
  }
}

function resolveShapeType(graph: SchemaGraph, type: SchemaType): SchemaTypeBody | undefined {
  let current = type;
  const seen = new Set<TypeId>();
  while (current.body.tag === 'ref') {
    if (seen.has(current.body.id)) return undefined;
    seen.add(current.body.id);
    const definition = graph.defs.get(current.body.id);
    if (!definition) return undefined;
    current = definition.body;
  }
  return current.body;
}

function optionalSchemaTypesMatch(
  leftGraph: SchemaGraph,
  left: SchemaType | undefined,
  rightGraph: SchemaGraph,
  right: SchemaType | undefined,
  depth: number,
  visiting: Map<TypeId, Set<TypeId>>,
): boolean {
  return left === undefined || right === undefined
    ? left === right
    : schemaTypesMatch(leftGraph, left, rightGraph, right, depth, visiting);
}

function stringArraysEqual(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function stringSetsEqual(left: readonly string[], right: readonly string[]): boolean {
  const leftSet = new Set(left);
  const rightSet = new Set(right);
  return leftSet.size === rightSet.size && [...leftSet].every((value) => rightSet.has(value));
}

function optionalStringSetsEqual(
  left: readonly string[] | undefined,
  right: readonly string[] | undefined,
): boolean {
  return left === undefined || right === undefined ? left === right : stringSetsEqual(left, right);
}

function effectiveQuantityUnits(spec: QuantitySpec): readonly string[] {
  return spec.allowedSuffixes.length === 0 ? [spec.baseUnit] : spec.allowedSuffixes;
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
  | { tag: 'secret'; handle: GuestSecretHandle }
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
  s8: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 's8', restrictions }),
  s16: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 's16', restrictions }),
  s32: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 's32', restrictions }),
  s64: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 's64', restrictions }),
  u8: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'u8', restrictions }),
  u16: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'u16', restrictions }),
  u32: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'u32', restrictions }),
  u64: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'u64', restrictions }),
  f32: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'f32', restrictions }),
  f64: (restrictions?: NumericRestrictions): SchemaType => schemaType({ tag: 'f64', restrictions }),
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
  path: (spec: PathSpec): SchemaType => schemaType({ tag: 'path', spec }),
  url: (restrictions: UrlRestrictions): SchemaType => schemaType({ tag: 'url', restrictions }),
  datetime: (): SchemaType => schemaType({ tag: 'datetime' }),
  duration: (): SchemaType => schemaType({ tag: 'duration' }),
  quantity: (spec: QuantitySpec): SchemaType => schemaType({ tag: 'quantity', spec }),
  secret: (inner: SchemaType, spec: Omit<SecretSpec, 'inner'> = {}): SchemaType =>
    schemaType({ tag: 'secret', spec, inner }),
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
  path: (value: string): SchemaValue => ({ tag: 'path', value }),
  url: (value: string): SchemaValue => ({ tag: 'url', value }),
  datetime: (value: Datetime): SchemaValue => ({ tag: 'datetime', value }),
  duration: (nanoseconds: bigint): SchemaValue => ({ tag: 'duration', nanoseconds }),
  quantity: (value: QuantityValue): SchemaValue => ({ tag: 'quantity', value }),
  secret: (handle: GuestSecretHandle): SchemaValue => ({ tag: 'secret', handle }),
  quotaToken: (handle: GuestQuotaTokenHandle): SchemaValue => ({ tag: 'quota-token', handle }),
};

/** Clone a schema value without duplicating affine capability handles. */
export function cloneSchemaValue(value: SchemaValue): SchemaValue {
  switch (value.tag) {
    case 'record':
      return { tag: 'record', fields: value.fields.map(cloneSchemaValue) };
    case 'variant':
      return {
        tag: 'variant',
        caseIndex: value.caseIndex,
        payload: value.payload ? cloneSchemaValue(value.payload) : undefined,
      };
    case 'flags':
      return { tag: 'flags', flags: [...value.flags] };
    case 'tuple':
      return { tag: 'tuple', elements: value.elements.map(cloneSchemaValue) };
    case 'list':
      return { tag: 'list', elements: value.elements.map(cloneSchemaValue) };
    case 'fixed-list':
      return { tag: 'fixed-list', elements: value.elements.map(cloneSchemaValue) };
    case 'map':
      return {
        tag: 'map',
        entries: value.entries.map((entry) => ({
          key: cloneSchemaValue(entry.key),
          value: cloneSchemaValue(entry.value),
        })),
      };
    case 'option':
      return {
        tag: 'option',
        value: value.value !== undefined ? cloneSchemaValue(value.value) : undefined,
      };
    case 'result':
      return {
        tag: 'result',
        result: {
          tag: value.result.tag,
          value:
            value.result.value !== undefined ? cloneSchemaValue(value.result.value) : undefined,
        },
      };
    case 'binary':
      return { ...value, bytes: value.bytes.slice() };
    case 'datetime':
      return { ...value, value: { ...value.value } };
    case 'quantity':
      return { ...value, value: { ...value.value } };
    case 'union':
      return { tag: 'union', unionTag: value.unionTag, body: cloneSchemaValue(value.body) };
    default:
      return { ...value };
  }
}

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
  if (a instanceof GuestSecretHandle || b instanceof GuestSecretHandle) return false;
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
