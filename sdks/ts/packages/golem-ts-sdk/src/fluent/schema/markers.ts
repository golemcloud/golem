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
import { buildResultCodec } from './result';
import { StandardSchemaV1 } from './standardSchema';
import { Result } from '../../host/result';
import { Principal, sdkPrincipalToHost, sdkPrincipalFromHost } from '../../principal';
import type {
  Principal as HostPrincipal,
  OidcPrincipal as HostOidcPrincipal,
  AgentId as HostAgentId,
  AccountId as HostAccountId,
} from 'golem:agent/common@2.0.0';

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
export type MarkerKind =
  | 'scalar'
  | 'multimodal'
  | 'unstructured-text'
  | 'unstructured-binary'
  | 'secret'
  | 'result'
  | 'typed-array'
  | 'principal';

/** Pure phantom key carrying a marker's {@link MarkerKind} at the type level. */
declare const MARKER_KIND: unique symbol;

/** A Standard Schema that additionally carries a {@link WIT_MARKER} brand. */
export interface MarkerSchema<
  Output = unknown,
  Kind extends MarkerKind = 'scalar',
> extends StandardSchemaV1<Output, Output> {
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

/**
 * Runtime brand key carried by every secret marker; its value is the marker's
 * INNER Standard Schema (describing the revealed plaintext). It doubles as the
 * type-level carrier of the inner type — {@link SecretInnerOf} recovers it — so
 * no separate phantom field is needed. Symbol-keyed, so it is invisible to
 * `JSON.stringify` / `Object.keys` and does not disturb the codec build.
 */
export const SECRET_INNER: unique symbol = Symbol('golem.secretInner');

/**
 * A secret marker: a {@link MarkerSchema} of kind `'secret'` that additionally
 * carries its inner (revealed) schema under {@link SECRET_INNER}. The static
 * `Inner` type is recoverable via {@link SecretInnerOf}.
 */
export interface SecretMarkerSchema<Inner> extends MarkerSchema<RawSecret, 'secret'> {
  readonly [SECRET_INNER]: StandardSchemaV1<Inner>;
}

/** Recover the inner (revealed) type of a secret marker, else `never`. */
export type SecretInnerOf<V> = V extends SecretMarkerSchema<infer I> ? I : never;

/** Type guard: is this value a secret marker (kind `'secret'`)? */
export function isSecretMarker(value: unknown): value is SecretMarkerSchema<unknown> {
  return isMarkerSchema(value) && SECRET_INNER in value;
}

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
      const n =
        typeof value === 'bigint' ? value : typeof value === 'number' ? BigInt(value) : undefined;
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
        ? (v[tag] as (x: bigint) => SchemaValue)(
            typeof value === 'bigint' ? value : BigInt(value as number),
          )
        : (v[tag] as (x: number) => SchemaValue)(value as number),
    fromValue: (sv) => (sv as { value: number | bigint }).value,
  });
  return marker(validate, descriptor);
}

function f32Marker(opts?: NumericOpts): MarkerSchema<number> {
  const restrictions = buildRestrictions('float-bits', opts);
  const validate: Validator<number> = (value) => {
    if (typeof value !== 'number') return fail('Expected a number for WIT f32');
    if (opts?.min !== undefined && value < Number(opts.min))
      return fail(`Value ${value} below min for WIT f32`);
    if (opts?.max !== undefined && value > Number(opts.max))
      return fail(`Value ${value} above max for WIT f32`);
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
    isDatetime(value)
      ? ok(value)
      : fail('Expected a { seconds: bigint, nanoseconds: number } datetime');
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

/**
 * A typed-array marker: a JS TypedArray (`Int32Array`, `Float64Array`, `Uint8Array`,
 * …) carried as a WIT `list<primN>`. Generalises the former `bytesMarker`: the
 * per-kind `spec` pins the concrete subclass (`ctor`), the element type node
 * (`elemType`, e.g. `t.s32`) and the element value builder (`elemValue`, e.g.
 * `v.s32`). Encode iterates the array → `v.list([...])`; decode reconstructs the
 * CONCRETE subclass via `new ctor(len)`. The 64-bit kinds (`BigInt64Array` /
 * `BigUint64Array`) carry `bigint` elements (`v.s64` / `v.u64`).
 */
function typedArrayMarker<TArr, E extends number | bigint>(spec: {
  ctor: { new (length: number): TArr; readonly name: string };
  elemType: () => SchemaType;
  elemValue: (x: E) => SchemaValue;
}): MarkerSchema<TArr, 'typed-array'> {
  const validate: Validator<TArr> = (value) =>
    value instanceof spec.ctor
      ? ok(value)
      : fail(`Expected a ${spec.ctor.name} for a typed-array WIT list`);
  const descriptor: MarkerDescriptor = () => ({
    graph: { defs: new Map(), root: t.list(spec.elemType()) },
    toValue: (value) => v.list(Array.from(value as Iterable<E>).map((x) => spec.elemValue(x))),
    fromValue: (sv) => {
      const elements = (sv as Extract<SchemaValue, { tag: 'list' }>).elements;
      const out = new spec.ctor(elements.length);
      elements.forEach((e, i) => {
        (out as Record<number, E>)[i] = (e as unknown as { value: E }).value;
      });
      return out;
    },
  });
  return marker<TArr, 'typed-array'>(validate, descriptor);
}

// ============================================================
// Capability wrapper: secret
// ============================================================

function secretMarker<Output>(inner: StandardSchemaV1<Output>): SecretMarkerSchema<Output> {
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
      toValue: (value) => v.secret(GuestSecretHandle.fromRaw(SECRET_INTERNAL, value as RawSecret)),
      fromValue: (sv) => {
        const handle = (sv as { tag: 'secret'; handle: GuestSecretHandle }).handle;
        const raw = handle.take();
        if (raw === undefined) {
          throw new Error(
            'secret handle was already consumed; an owned secret can only be decoded once',
          );
        }
        return raw;
      },
      // Expose the inner (revealed-value) codec so the config surface can decode
      // a secret leaf's plaintext after `golem:secrets/reveal`, at any depth.
      secretInner: innerCodec,
    };
  };
  // Carry the inner schema under SECRET_INNER so `compileConfig` can detect the
  // secret at runtime and recover the inner codec, and so `SecretInnerOf` can
  // recover the inner type. The symbol field does not affect the marker's codec
  // (compileSchema dispatches on WIT_MARKER before reading any other field).
  const base = marker<RawSecret, 'secret'>(validate, descriptor);
  return Object.assign(base, { [SECRET_INNER]: inner }) as SecretMarkerSchema<Output>;
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
          throw new Error(
            `Language code \`${p.language}\` is not allowed. Allowed: ${languages.join(', ')}`,
          );
        }
        return p.language
          ? { tag: 'inline', val: p.text, languageCode: p.language }
          : { tag: 'inline', val: p.text };
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
          throw new Error(
            `Mime type \`${p.mimeType}\` is not allowed. Allowed: ${mimeTypes.join(', ')}`,
          );
        }
        return p.mimeType
          ? { tag: 'inline', val: p.bytes, mimeType: p.mimeType }
          : { tag: 'inline', val: p.bytes };
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

function multimodalMarker(
  cases: MultimodalCase[],
): MarkerSchema<MultimodalElement[], 'multimodal'> {
  const validate: Validator<MultimodalElement[]> = (value) =>
    Array.isArray(value)
      ? ok(value as MultimodalElement[])
      : fail('Expected an array of multimodal elements');
  const descriptor: MarkerDescriptor = (recurse) => {
    const caseCodecs = cases.map((c) => ({ name: c.name, codec: recurse(c.schema) }));
    const byName = new Map(
      caseCodecs.map((c, i) => [c.name, { codec: c.codec, index: i }] as const),
    );
    const variantCases: VariantCaseType[] = caseCodecs.map((c) =>
      variantCase(c.name, c.codec.graph.root),
    );
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
          if (vv.payload === undefined)
            throw new Error(`multimodal: missing payload for case '${c.name}'`);
          return { tag: c.name, value: c.codec.fromValue(vv.payload) };
        });
      },
    };
  };
  return marker<MultimodalElement[], 'multimodal'>(validate, descriptor);
}

// ============================================================
// Result (WIT `result<ok, err>`)
// ============================================================

/** Runtime guard: is `value` an SDK {@link Result} (`Result.ok`/`Result.err`)? */
function isResultValue(value: unknown): value is Result<unknown, unknown> {
  return (
    typeof value === 'object' &&
    value !== null &&
    'tag' in value &&
    ((value as { tag: unknown }).tag === 'ok' || (value as { tag: unknown }).tag === 'err') &&
    'val' in value
  );
}

/**
 * A `result<ok, err>` marker. Its `returns`-schema output type is `Result<Ok, Err>`
 * (the SDK {@link Result}); the descriptor lowers it to a WIT `result-type` via
 * {@link buildResultCodec}. The handler returns `Result.ok(v)` / `Result.err(e)`
 * and the caller receives the decoded `Result<Ok, Err>` — the failure travels as a
 * value inside the success payload (matching the decorator SDK), NOT the WIT
 * `agent-error` channel.
 */
function resultMarker<Ok, Err>(
  okSchema: StandardSchemaV1<Ok>,
  errSchema: StandardSchemaV1<Err>,
): MarkerSchema<Result<Ok, Err>, 'result'> {
  const validate: Validator<Result<Ok, Err>> = (value) =>
    isResultValue(value)
      ? ok(value as Result<Ok, Err>)
      : fail('Expected a Result value (Result.ok(...) / Result.err(...))');
  const descriptor: MarkerDescriptor = (recurse) =>
    buildResultCodec(recurse(okSchema), recurse(errSchema));
  return marker<Result<Ok, Err>, 'result'>(validate, descriptor);
}

// ============================================================
// Principal (WIT `golem:agent/common` `principal` variant)
// ============================================================

// The SDK `Principal` (produced by `this.getPrincipal()`) carried as a WIT
// `principal` variant value. Unlike `this.getPrincipal()` (a capability read),
// this marker lets a `Principal` travel as ordinary structured data in a method
// param / return. The graph mirrors the host `Principal` shape from
// `golem:agent/common@2.0.0` exactly (case order oidc/agent/golem-user/anonymous),
// and the codec round-trips SDK `Principal` <-> `SchemaValue` via the host shape.

const PRINCIPAL_TAGS = ['oidc', 'agent', 'golem-user', 'anonymous'];

// --- graph type builders (no recursion: everything is built inline) ---
const uuidType = (): SchemaType =>
  t.record([field('highBits', t.u64()), field('lowBits', t.u64())]);
const componentIdType = (): SchemaType => t.record([field('uuid', uuidType())]);
const agentIdType = (): SchemaType =>
  t.record([field('componentId', componentIdType()), field('agentId', t.string())]);
const accountIdType = (): SchemaType => t.record([field('uuid', uuidType())]);
const oidcType = (): SchemaType =>
  t.record([
    field('sub', t.string()),
    field('issuer', t.string()),
    field('email', t.option(t.string())),
    field('name', t.option(t.string())),
    field('emailVerified', t.option(t.bool())),
    field('givenName', t.option(t.string())),
    field('familyName', t.option(t.string())),
    field('picture', t.option(t.string())),
    field('preferredUsername', t.option(t.string())),
    field('claims', t.string()),
  ]);
const agentPrincipalType = (): SchemaType => t.record([field('agentId', agentIdType())]);
const golemUserType = (): SchemaType => t.record([field('accountId', accountIdType())]);

// --- SchemaValue field accessors (positional record reads) ---
const recFields = (sv: SchemaValue): SchemaValue[] =>
  (sv as { tag: 'record'; fields: SchemaValue[] }).fields;
const u64Of = (f: SchemaValue): bigint => (f as { tag: 'u64'; value: bigint }).value;
const strOf = (f: SchemaValue): string => (f as { tag: 'string'; value: string }).value;
const boolOf = (f: SchemaValue): boolean => (f as { tag: 'bool'; value: boolean }).value;
const optOf = (f: SchemaValue): SchemaValue | undefined =>
  (f as { tag: 'option'; value?: SchemaValue }).value;

// --- codec helpers (round-trip via the host shape) ---
type HostUuid = { highBits: bigint; lowBits: bigint };

function uuidToValue(u: HostUuid): SchemaValue {
  return v.record([v.u64(u.highBits), v.u64(u.lowBits)]);
}
function uuidFromValue(sv: SchemaValue): HostUuid {
  const f = recFields(sv);
  return { highBits: u64Of(f[0]), lowBits: u64Of(f[1]) };
}

function agentIdToValue(a: HostAgentId): SchemaValue {
  return v.record([v.record([uuidToValue(a.componentId.uuid)]), v.string(a.agentId)]);
}
function agentIdFromValue(sv: SchemaValue): HostAgentId {
  const f = recFields(sv);
  const uuid = uuidFromValue(recFields(f[0])[0]);
  return { componentId: { uuid }, agentId: strOf(f[1]) };
}

function accountIdToValue(a: HostAccountId): SchemaValue {
  return v.record([uuidToValue(a.uuid)]);
}
function accountIdFromValue(sv: SchemaValue): HostAccountId {
  return { uuid: uuidFromValue(recFields(sv)[0]) };
}

function oidcToValue(o: HostOidcPrincipal): SchemaValue {
  const optStr = (x: string | undefined): SchemaValue =>
    v.option(x === undefined ? undefined : v.string(x));
  return v.record([
    v.string(o.sub),
    v.string(o.issuer),
    optStr(o.email),
    optStr(o.name),
    v.option(o.emailVerified === undefined ? undefined : v.bool(o.emailVerified)),
    optStr(o.givenName),
    optStr(o.familyName),
    optStr(o.picture),
    optStr(o.preferredUsername),
    v.string(o.claims),
  ]);
}
function oidcFromValue(sv: SchemaValue): HostOidcPrincipal {
  const f = recFields(sv);
  const optStr = (field: SchemaValue): string | undefined => {
    const val = optOf(field);
    return val === undefined ? undefined : strOf(val);
  };
  const out: HostOidcPrincipal = { sub: strOf(f[0]), issuer: strOf(f[1]), claims: strOf(f[9]) };
  const email = optStr(f[2]);
  if (email !== undefined) out.email = email;
  const name = optStr(f[3]);
  if (name !== undefined) out.name = name;
  const ev = optOf(f[4]);
  if (ev !== undefined) out.emailVerified = boolOf(ev);
  const givenName = optStr(f[5]);
  if (givenName !== undefined) out.givenName = givenName;
  const familyName = optStr(f[6]);
  if (familyName !== undefined) out.familyName = familyName;
  const picture = optStr(f[7]);
  if (picture !== undefined) out.picture = picture;
  const preferredUsername = optStr(f[8]);
  if (preferredUsername !== undefined) out.preferredUsername = preferredUsername;
  return out;
}

function principalMarker(): MarkerSchema<Principal, 'principal'> {
  const validate: Validator<Principal> = (value) =>
    value !== null &&
    typeof value === 'object' &&
    typeof (value as { tag?: unknown }).tag === 'string' &&
    PRINCIPAL_TAGS.includes((value as { tag: string }).tag)
      ? ok(value as Principal)
      : fail('Expected a Principal (e.g. from this.getPrincipal())');
  const descriptor: MarkerDescriptor = () => ({
    graph: {
      defs: new Map(),
      root: t.variant([
        variantCase('oidc', oidcType()),
        variantCase('agent', agentPrincipalType()),
        variantCase('golem-user', golemUserType()),
        variantCase('anonymous'),
      ]),
    },
    toValue: (value) => {
      const h = sdkPrincipalToHost(value as Principal);
      switch (h.tag) {
        case 'oidc':
          return v.variant(0, oidcToValue(h.val));
        case 'agent':
          return v.variant(1, v.record([agentIdToValue(h.val.agentId)]));
        case 'golem-user':
          return v.variant(2, v.record([accountIdToValue(h.val.accountId)]));
        case 'anonymous':
          return v.variant(3);
      }
    },
    fromValue: (sv) => {
      const vv = sv as { tag: 'variant'; caseIndex: number; payload?: SchemaValue };
      let host: HostPrincipal;
      switch (vv.caseIndex) {
        case 0:
          host = { tag: 'oidc', val: oidcFromValue(vv.payload as SchemaValue) };
          break;
        case 1:
          host = {
            tag: 'agent',
            val: { agentId: agentIdFromValue(recFields(vv.payload as SchemaValue)[0]) },
          };
          break;
        case 2:
          host = {
            tag: 'golem-user',
            val: { accountId: accountIdFromValue(recFields(vv.payload as SchemaValue)[0]) },
          };
          break;
        default:
          host = { tag: 'anonymous' };
      }
      return sdkPrincipalFromHost(host);
    },
  });
  return marker<Principal, 'principal'>(validate, descriptor);
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
  u32: (opts?: NumericOpts) =>
    intMarker({ tag: 'u32', min: 0n, max: 4294967295n, big: false }, opts),
  u64: (opts?: NumericOpts) =>
    intMarker({ tag: 'u64', min: 0n, max: 18446744073709551615n, big: true }, opts),
  s8: (opts?: NumericOpts) => intMarker({ tag: 's8', min: -128n, max: 127n, big: false }, opts),
  s16: (opts?: NumericOpts) =>
    intMarker({ tag: 's16', min: -32768n, max: 32767n, big: false }, opts),
  s32: (opts?: NumericOpts) =>
    intMarker({ tag: 's32', min: -2147483648n, max: 2147483647n, big: false }, opts),
  s64: (opts?: NumericOpts) =>
    intMarker(
      { tag: 's64', min: -9223372036854775808n, max: 9223372036854775807n, big: true },
      opts,
    ),
  f32: (opts?: NumericOpts) => f32Marker(opts),

  // Scalars Standard Schema can't pin.
  char: () => charMarker(),
  datetime: () => datetimeMarker(),
  duration: () => durationMarker(),
  url: () => urlMarker(),
  bytes: () => typedArrayMarker({ ctor: Uint8Array, elemType: t.u8, elemValue: v.u8 }),

  // Typed arrays: each JS TypedArray kind → WIT `list<primN>`, decoded to the
  // concrete subclass. `s.bytes()` above is the `Uint8Array` alias.
  int8Array: () => typedArrayMarker({ ctor: Int8Array, elemType: t.s8, elemValue: v.s8 }),
  uint8Array: () => typedArrayMarker({ ctor: Uint8Array, elemType: t.u8, elemValue: v.u8 }),
  int16Array: () => typedArrayMarker({ ctor: Int16Array, elemType: t.s16, elemValue: v.s16 }),
  uint16Array: () => typedArrayMarker({ ctor: Uint16Array, elemType: t.u16, elemValue: v.u16 }),
  int32Array: () => typedArrayMarker({ ctor: Int32Array, elemType: t.s32, elemValue: v.s32 }),
  uint32Array: () => typedArrayMarker({ ctor: Uint32Array, elemType: t.u32, elemValue: v.u32 }),
  float32Array: () => typedArrayMarker({ ctor: Float32Array, elemType: t.f32, elemValue: v.f32 }),
  float64Array: () => typedArrayMarker({ ctor: Float64Array, elemType: t.f64, elemValue: v.f64 }),
  bigInt64Array: () => typedArrayMarker({ ctor: BigInt64Array, elemType: t.s64, elemValue: v.s64 }),
  bigUint64Array: () =>
    typedArrayMarker({ ctor: BigUint64Array, elemType: t.u64, elemValue: v.u64 }),

  // Capability wrappers / nodes.
  secret: <Output>(inner: StandardSchemaV1<Output>) => secretMarker(inner),
  quotaToken: () => quotaTokenMarker(),

  // Principal carried as a data value (WIT `principal` variant).
  principal: () => principalMarker(),

  // Rich nodes.
  multimodal: (cases: MultimodalCase[]) => multimodalMarker(cases),
  unstructuredText: (opts?: UnstructuredTextOpts) => unstructuredTextMarker(opts),
  unstructuredBinary: (opts?: UnstructuredBinaryOpts) => unstructuredBinaryMarker(opts),

  // Typed method result: `returns: s.result(ok, err)` → WIT `result<ok, err>`.
  result: <Ok, Err>(okSchema: StandardSchemaV1<Ok>, errSchema: StandardSchemaV1<Err>) =>
    resultMarker(okSchema, errSchema),
};
