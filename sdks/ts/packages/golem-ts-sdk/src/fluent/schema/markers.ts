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

// Vendor-neutral "marker" schemas. Standard Schema can express scalar shapes
// (string/number/boolean/…) but has NO way to pin a numeric to a specific WIT
// width, to say "this string is a single `char`", or to select the capability /
// rich nodes (secret, quota-token, multimodal, unstructured). A marker fills
// that gap by carrying the extra WIT intent alongside a plain Standard Schema.
//
// A marker is a fully-valid {@link StandardSchemaV1} (so it passes the existing
// param/return typing and `isStandardSchema` checks) that ALSO carries a hidden
// brand under the {@link WIT_MARKER} symbol key. The brand is a
// `MarkerDescriptor`: a thunk that, given a `recurse` callback for inner
// schemas, builds the {@link FluentCodec}. `compileSchema` checks for this brand
// BEFORE the vendor dispatch and, when present, builds the codec from the
// descriptor — so markers never reach a vendor walker.

import {
  emptyMetadata,
  field,
  mergeGraphDefs,
  NamedFieldType,
  schemaType,
  SchemaType,
  SchemaValue,
  t,
  v,
  variantCase,
  VariantCaseType,
  type Datetime,
  type NumericBound,
  type NumericRestrictions,
  type Role,
} from '../../internal/schema-model';
import { GuestSecretHandle } from '../../internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../../internal/schema-model/secretInternal';
import { GuestQuotaTokenHandle } from '../../internal/schema-model/quotaTokenHandle';
import { QUOTA_INTERNAL } from '../../internal/schema-model/quotaInternal';
import type { Secret as RawSecret, QuotaToken as RawQuotaToken } from 'golem:core/types@2.0.0';
import { FluentCodec } from './codec';
import { StandardSchemaV1 } from './standardSchema';

/**
 * Hidden brand key carried by every marker schema. The value is a
 * {@link MarkerDescriptor} the adapter uses to build the {@link FluentCodec}.
 * A unique, unexported `Symbol` cannot be forged or guessed, so only this module
 * mints markers and only the adapter reads the brand.
 */
export const WIT_MARKER: unique symbol = Symbol('golem.witMarker');

/**
 * Build a {@link FluentCodec} from a marker. `recurse` compiles an inner
 * Standard Schema (the same `compileSchema` callback the vendor walkers use), so
 * capability wrappers (e.g. `s.secret(inner)`) can descend into their payload.
 */
export type MarkerDescriptor = (recurse: (child: unknown) => FluentCodec) => FluentCodec;

/**
 * Type-level classification of a marker. `'scalar'` covers everything that can
 * be bound from a string source (numeric pins, `char`, `datetime`, `url`,
 * `secret`, …); the other three tag the rich payload markers that can NOT be
 * bound from a path / query / header variable and therefore must be excluded
 * from HTTP variable binding (see {@link MarkerKindOf} / `BindableKeys`).
 */
export type MarkerKind = 'scalar' | 'multimodal' | 'unstructured-text' | 'unstructured-binary';

/** Pure phantom key carrying a marker's {@link MarkerKind} at the type level. */
declare const MARKER_KIND: unique symbol;

/** A Standard Schema that additionally carries a {@link WIT_MARKER} brand. */
export interface MarkerSchema<Output = unknown, Kind extends MarkerKind = 'scalar'>
  extends StandardSchemaV1<Output, Output> {
  readonly [WIT_MARKER]: MarkerDescriptor;
  /**
   * PURE PHANTOM: never assigned at runtime (no runtime object carries it), so
   * marker values are structurally unchanged. It exists only so the marker's
   * {@link MarkerKind} is recoverable at the type level via {@link MarkerKindOf}.
   */
  readonly [MARKER_KIND]?: Kind;
}

/**
 * Recover a marker's {@link MarkerKind} at the type level. Resolves to
 * `undefined` for any value that is not a {@link MarkerSchema} (e.g. a plain
 * Zod / Valibot / ArkType schema).
 */
export type MarkerKindOf<V> = V extends MarkerSchema<any, infer K> ? K : undefined;

/** Type guard: does this value carry a {@link WIT_MARKER} brand? */
export function isMarkerSchema(value: unknown): value is MarkerSchema {
  return (
    typeof value === 'object' &&
    value !== null &&
    WIT_MARKER in (value as Record<PropertyKey, unknown>) &&
    typeof (value as Record<PropertyKey, unknown>)[WIT_MARKER] === 'function'
  );
}

// ============================================================
// Marker construction helpers
// ============================================================

type Validator<Output> = (value: unknown) => StandardSchemaV1.Result<Output>;

/**
 * Mint a marker: a Standard Schema whose `~standard.validate` runs `validate`,
 * branded with the codec-builder `descriptor`. The `vendor` is informational
 * (markers are intercepted before vendor dispatch).
 */
function marker<Output, Kind extends MarkerKind = 'scalar'>(
  validate: Validator<Output>,
  descriptor: MarkerDescriptor,
): MarkerSchema<Output, Kind> {
  return {
    '~standard': {
      version: 1,
      vendor: 'golem.marker',
      validate,
    },
    [WIT_MARKER]: descriptor,
  };
}

function ok<Output>(value: Output): StandardSchemaV1.Result<Output> {
  return { value };
}

function fail(message: string): StandardSchemaV1.FailureResult {
  return { issues: [{ message }] };
}

// ============================================================
// Numeric restrictions (inline min/max/unit bounds)
// ============================================================

/** Options for pinning a numeric with inline min/max/unit restrictions. */
export interface NumericOpts {
  min?: number | bigint;
  max?: number | bigint;
  unit?: string;
}

const F64_BITS_VIEW = new DataView(new ArrayBuffer(8));
/** Canonical IEEE-754 f64 bits of `x` as a u64 (the `float-bits` bound payload). */
function f64Bits(x: number): bigint {
  // Canonicalize -0.0 to +0.0 so equal bounds compare equal (mirrors the codec).
  F64_BITS_VIEW.setFloat64(0, x === 0 ? 0 : x);
  return F64_BITS_VIEW.getBigUint64(0);
}

type BoundKind = 'unsigned' | 'signed' | 'float-bits';

function makeBound(kind: BoundKind, x: number | bigint): NumericBound {
  return kind === 'float-bits'
    ? { tag: 'float-bits', val: f64Bits(Number(x)) }
    : { tag: kind, val: BigInt(x) };
}

/** Build `NumericRestrictions` from user opts, or `undefined` when empty (= unconstrained). */
function buildRestrictions(kind: BoundKind, opts?: NumericOpts): NumericRestrictions | undefined {
  if (!opts || (opts.min === undefined && opts.max === undefined && !opts.unit)) return undefined;
  return {
    min: opts.min !== undefined ? makeBound(kind, opts.min) : undefined,
    max: opts.max !== undefined ? makeBound(kind, opts.max) : undefined,
    unit: opts.unit,
  };
}

// ============================================================
// Scalar pins
// ============================================================

interface IntPin {
  /** WIT body tag, also the matching `t.*` / `v.*` key. */
  tag: 'u8' | 'u16' | 'u32' | 'u64' | 's8' | 's16' | 's32' | 's64';
  min: bigint;
  max: bigint;
  /** Whether the runtime value is a `bigint` (64-bit) or a `number`. */
  big: boolean;
}

function intMarker(pin: IntPin, opts?: NumericOpts): MarkerSchema<number | bigint> {
  const { tag, min: typeMin, max: typeMax, big } = pin;
  const kind: BoundKind = tag.startsWith('s') ? 'signed' : 'unsigned';
  const restrictions = buildRestrictions(kind, opts);
  // Effective bounds tighten the type range with the user's min/max.
  const lo = opts?.min !== undefined && BigInt(opts.min) > typeMin ? BigInt(opts.min) : typeMin;
  const hi = opts?.max !== undefined && BigInt(opts.max) < typeMax ? BigInt(opts.max) : typeMax;
  const validate: Validator<number | bigint> = (value) => {
    if (big) {
      const n = typeof value === 'bigint' ? value : typeof value === 'number' ? BigInt(value) : undefined;
      if (n === undefined) return fail(`Expected a bigint or number for WIT ${tag}`);
      if (n < lo || n > hi) return fail(`Value ${n} out of range for WIT ${tag}`);
      return ok(n);
    }
    if (typeof value !== 'number' || !Number.isInteger(value)) {
      return fail(`Expected an integer for WIT ${tag}`);
    }
    if (BigInt(value) < lo || BigInt(value) > hi) {
      return fail(`Value ${value} out of range for WIT ${tag}`);
    }
    return ok(value);
  };
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t[tag](restrictions) },
    toValue: (value) =>
      big
        ? (v[tag] as (x: bigint) => SchemaValue)(typeof value === 'bigint' ? value : BigInt(value as number))
        : (v[tag] as (x: number) => SchemaValue)(value as number),
    fromValue: (sv) => (sv as { value: number | bigint }).value,
  });
  return marker(validate, descriptor);
}

function f32Marker(opts?: NumericOpts): MarkerSchema<number> {
  const restrictions = buildRestrictions('float-bits', opts);
  const validate: Validator<number> = (value) => {
    if (typeof value !== 'number') return fail('Expected a number for WIT f32');
    if (opts?.min !== undefined && value < Number(opts.min)) return fail(`Value ${value} below min for WIT f32`);
    if (opts?.max !== undefined && value > Number(opts.max)) return fail(`Value ${value} above max for WIT f32`);
    return ok(value);
  };
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.f32(restrictions) },
    toValue: (value) => v.f32(value as number),
    fromValue: (sv) => (sv as { tag: 'f32'; value: number }).value,
  });
  return marker(validate, descriptor);
}

// ============================================================
// char / datetime / duration / url / bytes
// ============================================================

function charMarker(): MarkerSchema<string> {
  const validate: Validator<string> = (value) =>
    typeof value === 'string' && Array.from(value).length === 1
      ? ok(value)
      : fail('Expected a single-character string for WIT char');
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.char() },
    toValue: (value) => v.char(value as string),
    fromValue: (sv) => (sv as { tag: 'char'; value: string }).value,
  });
  return marker(validate, descriptor);
}

function isDatetime(value: unknown): value is Datetime {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Datetime).seconds === 'bigint' &&
    typeof (value as Datetime).nanoseconds === 'number'
  );
}

function datetimeMarker(): MarkerSchema<Datetime> {
  const validate: Validator<Datetime> = (value) =>
    isDatetime(value) ? ok(value) : fail('Expected a { seconds: bigint, nanoseconds: number } datetime');
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.datetime() },
    toValue: (value) => v.datetime(value as Datetime),
    fromValue: (sv) => (sv as { tag: 'datetime'; value: Datetime }).value,
  });
  return marker(validate, descriptor);
}

function durationMarker(): MarkerSchema<bigint> {
  const validate: Validator<bigint> = (value) => {
    if (typeof value === 'bigint') return ok(value);
    if (typeof value === 'number' && Number.isInteger(value)) return ok(BigInt(value));
    return fail('Expected a bigint (nanoseconds) for WIT duration');
  };
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.duration() },
    toValue: (value) => v.duration(typeof value === 'bigint' ? value : BigInt(value as number)),
    fromValue: (sv) => (sv as { tag: 'duration'; nanoseconds: bigint }).nanoseconds,
  });
  return marker(validate, descriptor);
}

function urlMarker(): MarkerSchema<string> {
  const validate: Validator<string> = (value) =>
    typeof value === 'string' ? ok(value) : fail('Expected a string URL for WIT url');
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.url({}) },
    toValue: (value) => v.url(value as string),
    fromValue: (sv) => (sv as { tag: 'url'; value: string }).value,
  });
  return marker(validate, descriptor);
}

function bytesMarker(): MarkerSchema<Uint8Array> {
  const validate: Validator<Uint8Array> = (value) =>
    value instanceof Uint8Array ? ok(value) : fail('Expected a Uint8Array for WIT list<u8>');
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.list(t.u8()) },
    toValue: (value) => v.list(Array.from(value as Uint8Array).map((b) => v.u8(b))),
    fromValue: (sv) => {
      const elements = (sv as Extract<SchemaValue, { tag: 'list' }>).elements;
      const out = new Uint8Array(elements.length);
      elements.forEach((e, i) => {
        out[i] = (e as { tag: 'u8'; value: number }).value;
      });
      return out;
    },
  });
  return marker(validate, descriptor);
}

// ============================================================
// Capability wrapper: secret
// ============================================================

function secretMarker<Output>(inner: StandardSchemaV1<Output>): MarkerSchema<RawSecret> {
  // The runtime secret value is an opaque owned `secret` resource handle, not
  // the revealed plaintext — so `validate` only checks it is a non-null object
  // (the inner schema constrains the *revealed* type, surfaced on the wire as
  // `secret<inner>`). The inner schema is recursed to build the inner WIT type.
  const validate: Validator<RawSecret> = (value) =>
    value !== null && typeof value === 'object'
      ? ok(value as RawSecret)
      : fail('Expected an opaque secret handle for WIT secret');
  const descriptor: MarkerDescriptor = (recurse) => {
    const innerCodec = recurse(inner);
    return {
      graph: { ...innerCodec.graph, root: t.secret(innerCodec.graph.root) },
      // Encode: wrap the freshly received owned `secret` resource in a
      // take-once handle. Decode: move the owned handle back out (take once).
      toValue: (value) =>
        v.secret(GuestSecretHandle.fromRaw(SECRET_INTERNAL, value as RawSecret)),
      fromValue: (sv) => {
        const handle = (sv as { tag: 'secret'; handle: GuestSecretHandle }).handle;
        const raw = handle.take();
        if (raw === undefined) {
          throw new Error('secret handle was already consumed; an owned secret can only be decoded once');
        }
        return raw;
      },
    };
  };
  return marker(validate, descriptor);
}

// ============================================================
// Capability node: quota-token
// ============================================================

function quotaTokenMarker(): MarkerSchema<RawQuotaToken> {
  const validate: Validator<RawQuotaToken> = (value) =>
    value !== null && typeof value === 'object'
      ? ok(value as RawQuotaToken)
      : fail('Expected an opaque quota-token handle for WIT quota-token');
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.quotaToken({}) },
    toValue: (value) =>
      v.quotaToken(GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, value as RawQuotaToken)),
    fromValue: (sv) => {
      const handle = (sv as { tag: 'quota-token'; handle: GuestQuotaTokenHandle }).handle;
      const raw = handle.take();
      if (raw === undefined) {
        throw new Error(
          'quota-token handle was already consumed; an owned quota-token can only be decoded once',
        );
      }
      return raw;
    },
  });
  return marker(validate, descriptor);
}

// ============================================================
// Rich nodes: unstructured text / binary
// ============================================================

const UNSTRUCTURED_TEXT_ROLE: Role = { tag: 'unstructured-text' };
const UNSTRUCTURED_BINARY_ROLE: Role = { tag: 'unstructured-binary' };
const MULTIMODAL_ROLE: Role = { tag: 'multimodal' };

// Variant case indices shared by unstructured text/binary (mirrors rich.ts).
const INLINE_CASE = 0;
const URL_CASE = 1;

/** Domain value of an unstructured-text parameter: `url` or `inline` text. */
export type TextReferenceValue =
  | { tag: 'url'; val: string }
  | { tag: 'inline'; val: string; languageCode?: string };

/** Domain value of an unstructured-binary parameter: `url` or `inline` bytes. */
export type BinaryReferenceValue =
  | { tag: 'url'; val: string }
  | { tag: 'inline'; val: Uint8Array; mimeType?: string };

interface UnstructuredTextOpts {
  languages?: string[];
}
interface UnstructuredBinaryOpts {
  mimeTypes?: string[];
}

function unstructuredTextMarker(
  opts?: UnstructuredTextOpts,
): MarkerSchema<TextReferenceValue, 'unstructured-text'> {
  const languages = opts?.languages ?? [];
  const validate: Validator<TextReferenceValue> = (value) =>
    value !== null && typeof value === 'object' && 'tag' in (value as object)
      ? ok(value as TextReferenceValue)
      : fail('Expected an unstructured-text reference ({ tag: "url" | "inline", … })');
  const descriptor: MarkerDescriptor = () => {
    const restrictions = languages.length > 0 ? { languages: [...languages] } : {};
    const variant = t.variant([
      variantCase('inline', schemaType({ tag: 'text', restrictions })),
      variantCase('url', schemaType({ tag: 'url', restrictions: {} })),
    ]);
    const root: SchemaType = {
      body: variant.body,
      metadata: { ...emptyMetadata(), role: UNSTRUCTURED_TEXT_ROLE },
    };
    return {
      graph: { defs: new Map(), root },
      toValue: (value) => {
        const ref = value as TextReferenceValue;
        if (ref.tag === 'url') return v.variant(URL_CASE, { tag: 'url', value: ref.val });
        return v.variant(INLINE_CASE, { tag: 'text', text: ref.val, language: ref.languageCode });
      },
      fromValue: (sv) => {
        const vv = sv as Extract<SchemaValue, { tag: 'variant' }>;
        if (vv.caseIndex === URL_CASE) {
          const p = vv.payload as Extract<SchemaValue, { tag: 'url' }>;
          return { tag: 'url', val: p.value };
        }
        const p = vv.payload as Extract<SchemaValue, { tag: 'text' }>;
        if (languages.length > 0 && p.language && !languages.includes(p.language)) {
          throw new Error(`Language code \`${p.language}\` is not allowed. Allowed: ${languages.join(', ')}`);
        }
        return p.language ? { tag: 'inline', val: p.text, languageCode: p.language } : { tag: 'inline', val: p.text };
      },
    };
  };
  return marker<TextReferenceValue, 'unstructured-text'>(validate, descriptor);
}

function unstructuredBinaryMarker(
  opts?: UnstructuredBinaryOpts,
): MarkerSchema<BinaryReferenceValue, 'unstructured-binary'> {
  const mimeTypes = opts?.mimeTypes ?? [];
  const validate: Validator<BinaryReferenceValue> = (value) =>
    value !== null && typeof value === 'object' && 'tag' in (value as object)
      ? ok(value as BinaryReferenceValue)
      : fail('Expected an unstructured-binary reference ({ tag: "url" | "inline", … })');
  const descriptor: MarkerDescriptor = () => {
    const restrictions = mimeTypes.length > 0 ? { mimeTypes: [...mimeTypes] } : {};
    const variant = t.variant([
      variantCase('inline', schemaType({ tag: 'binary', restrictions })),
      variantCase('url', schemaType({ tag: 'url', restrictions: {} })),
    ]);
    const root: SchemaType = {
      body: variant.body,
      metadata: { ...emptyMetadata(), role: UNSTRUCTURED_BINARY_ROLE },
    };
    return {
      graph: { defs: new Map(), root },
      toValue: (value) => {
        const ref = value as BinaryReferenceValue;
        if (ref.tag === 'url') return v.variant(URL_CASE, { tag: 'url', value: ref.val });
        return v.variant(INLINE_CASE, { tag: 'binary', bytes: ref.val, mimeType: ref.mimeType });
      },
      fromValue: (sv) => {
        const vv = sv as Extract<SchemaValue, { tag: 'variant' }>;
        if (vv.caseIndex === URL_CASE) {
          const p = vv.payload as Extract<SchemaValue, { tag: 'url' }>;
          return { tag: 'url', val: p.value };
        }
        const p = vv.payload as Extract<SchemaValue, { tag: 'binary' }>;
        if (mimeTypes.length > 0 && p.mimeType && !mimeTypes.includes(p.mimeType)) {
          throw new Error(`Mime type \`${p.mimeType}\` is not allowed. Allowed: ${mimeTypes.join(', ')}`);
        }
        return p.mimeType ? { tag: 'inline', val: p.bytes, mimeType: p.mimeType } : { tag: 'inline', val: p.bytes };
      },
    };
  };
  return marker<BinaryReferenceValue, 'unstructured-binary'>(validate, descriptor);
}

// ============================================================
// Rich node: multimodal
// ============================================================

/** Domain value element of a multimodal payload: `{ tag, value }`. */
export interface MultimodalElement {
  tag: string;
  value: unknown;
}

/** One named multimodal case: a name plus the marker / Standard Schema for it. */
export interface MultimodalCase {
  name: string;
  schema: unknown;
}

function multimodalMarker(cases: MultimodalCase[]): MarkerSchema<MultimodalElement[], 'multimodal'> {
  const validate: Validator<MultimodalElement[]> = (value) =>
    Array.isArray(value) ? ok(value as MultimodalElement[]) : fail('Expected an array of multimodal elements');
  const descriptor: MarkerDescriptor = (recurse) => {
    const caseCodecs = cases.map((c) => ({ name: c.name, codec: recurse(c.schema) }));
    const byName = new Map(caseCodecs.map((c, i) => [c.name, { codec: c.codec, index: i }] as const));
    const variantCases: VariantCaseType[] = caseCodecs.map((c) => variantCase(c.name, c.codec.graph.root));
    const variant = t.variant(variantCases);
    const defs = mergeGraphDefs(caseCodecs.map((c) => c.codec.graph));
    const root: SchemaType = {
      body: t.list(variant).body,
      metadata: { ...emptyMetadata(), role: MULTIMODAL_ROLE },
    };
    return {
      graph: { defs, root },
      toValue: (value) => {
        const elements = (value as MultimodalElement[]).map((item) => {
          const entry = byName.get(item.tag);
          if (entry === undefined) throw new Error(`multimodal: unknown case '${item.tag}'`);
          return v.variant(entry.index, entry.codec.toValue(item.value));
        });
        return v.list(elements);
      },
      fromValue: (sv) => {
        const list = sv as Extract<SchemaValue, { tag: 'list' }>;
        return list.elements.map((el) => {
          const vv = el as Extract<SchemaValue, { tag: 'variant' }>;
          const c = caseCodecs[vv.caseIndex];
          if (c === undefined) throw new Error(`multimodal: unknown case index ${vv.caseIndex}`);
          if (vv.payload === undefined) throw new Error(`multimodal: missing payload for case '${c.name}'`);
          return { tag: c.name, value: c.codec.fromValue(vv.payload) };
        });
      },
    };
  };
  return marker<MultimodalElement[], 'multimodal'>(validate, descriptor);
}

// ============================================================
// Public `s` namespace
// ============================================================

/**
 * Vendor-neutral marker schemas for WIT kinds Standard Schema can't express on
 * its own: numeric pins, `char`, `datetime`, `duration`, `url`, `bytes`, plus
 * the capability / rich nodes (`secret`, `quota-token`, `multimodal`,
 * `unstructured*`). Each returns a {@link MarkerSchema} usable anywhere a
 * Standard Schema is accepted (method params/returns, `id` fields).
 */
export const s = {
  // Numeric pins (f64 is the default `number`, so it is intentionally absent).
  u8: (opts?: NumericOpts) => intMarker({ tag: 'u8', min: 0n, max: 255n, big: false }, opts),
  u16: (opts?: NumericOpts) => intMarker({ tag: 'u16', min: 0n, max: 65535n, big: false }, opts),
  u32: (opts?: NumericOpts) => intMarker({ tag: 'u32', min: 0n, max: 4294967295n, big: false }, opts),
  u64: (opts?: NumericOpts) =>
    intMarker({ tag: 'u64', min: 0n, max: 18446744073709551615n, big: true }, opts),
  s8: (opts?: NumericOpts) => intMarker({ tag: 's8', min: -128n, max: 127n, big: false }, opts),
  s16: (opts?: NumericOpts) => intMarker({ tag: 's16', min: -32768n, max: 32767n, big: false }, opts),
  s32: (opts?: NumericOpts) =>
    intMarker({ tag: 's32', min: -2147483648n, max: 2147483647n, big: false }, opts),
  s64: (opts?: NumericOpts) =>
    intMarker({ tag: 's64', min: -9223372036854775808n, max: 9223372036854775807n, big: true }, opts),
  f32: (opts?: NumericOpts) => f32Marker(opts),

  // Scalars Standard Schema can't pin.
  char: () => charMarker(),
  datetime: () => datetimeMarker(),
  duration: () => durationMarker(),
  url: () => urlMarker(),
  bytes: () => bytesMarker(),

  // Capability wrappers / nodes.
  secret: <Output>(inner: StandardSchemaV1<Output>) => secretMarker(inner),
  quotaToken: () => quotaTokenMarker(),

  // Rich nodes.
  multimodal: (cases: MultimodalCase[]) => multimodalMarker(cases),
  unstructuredText: (opts?: UnstructuredTextOpts) => unstructuredTextMarker(opts),
  unstructuredBinary: (opts?: UnstructuredBinaryOpts) => unstructuredBinaryMarker(opts),
};
