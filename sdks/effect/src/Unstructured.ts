/**
 * @since 1.5.0
 */
import { Effect, Option, Schema, SchemaGetter, SchemaIssue } from "effect"
import type { Role } from "golem:core/types@2.0.0"
import {
  emptyMetadata,
  schemaType,
  t,
  v,
  variantCase,
  type SchemaType,
  type SchemaValue,
} from "./internal/schema-model/model.js"
import type { WitCodec } from "./WitCodec.js"

// ---------- Domain reference value shapes (mirror the new schema model) ----------

/**
 * Domain-side value of an unstructured-text parameter — either a `url`
 * (a plain string URL) or `inline` text carrying the data and an optional
 * BCP-47 `languageCode`.
 *
 * @since 1.5.0
 * @category models
 */
export type TextReferenceValue =
  | { readonly _tag: "url"; readonly val: string }
  | { readonly _tag: "inline"; readonly val: string; readonly languageCode?: string }

/**
 * Domain-side value of an unstructured-binary parameter — either a `url`
 * or `inline` binary data carrying the bytes and an optional `mimeType`.
 *
 * @since 1.5.0
 * @category models
 */
export type BinaryReferenceValue =
  | { readonly _tag: "url"; readonly val: string }
  | { readonly _tag: "inline"; readonly val: Uint8Array; readonly mimeType?: string }

const UNSTRUCTURED_TEXT_ROLE: Role = { tag: "unstructured-text" }
const UNSTRUCTURED_BINARY_ROLE: Role = { tag: "unstructured-binary" }

// Variant case indices shared by unstructured text/binary.
const INLINE_CASE = 0
const URL_CASE = 1

// ---------- Schema-type builders ----------

/**
 * `variant { inline: text, url: url }` tagged `role = unstructured-text`.
 *
 * @since 1.5.0
 * @category schema
 */
export const unstructuredTextSchemaType = (languages: ReadonlyArray<string>): SchemaType => {
  const restrictions = languages.length > 0 ? { languages: [...languages] } : {}
  const variant = t.variant([
    variantCase("inline", schemaType({ tag: "text", restrictions })),
    variantCase("url", schemaType({ tag: "url", restrictions: {} })),
  ])
  return { body: variant.body, metadata: { ...emptyMetadata(), role: UNSTRUCTURED_TEXT_ROLE } }
}

/**
 * `variant { inline: binary, url: url }` tagged `role = unstructured-binary`.
 *
 * @since 1.5.0
 * @category schema
 */
export const unstructuredBinarySchemaType = (mimeTypes: ReadonlyArray<string>): SchemaType => {
  const restrictions = mimeTypes.length > 0 ? { mimeTypes: [...mimeTypes] } : {}
  const variant = t.variant([
    variantCase("inline", schemaType({ tag: "binary", restrictions })),
    variantCase("url", schemaType({ tag: "url", restrictions: {} })),
  ])
  return { body: variant.body, metadata: { ...emptyMetadata(), role: UNSTRUCTURED_BINARY_ROLE } }
}

// ---------- Value codecs ----------

/** Encode a `TextReferenceValue` into the `variant { inline, url }` value. */
export const unstructuredTextToValue = (value: TextReferenceValue): SchemaValue => {
  if (value._tag === "url") {
    return v.variant(URL_CASE, { tag: "url", value: value.val })
  }
  return v.variant(INLINE_CASE, { tag: "text", text: value.val, language: value.languageCode })
}

/**
 * Decode a `variant { inline, url }` value into a `TextReferenceValue`.
 * Lenient: a missing language is always allowed; only a *present* language
 * outside a non-empty allow-list is rejected.
 */
export const unstructuredTextFromValue = (
  context: string,
  value: SchemaValue,
  allowedCodes: ReadonlyArray<string>,
): TextReferenceValue => {
  if (value.tag !== "variant") {
    throw new Error(`Expected variant value for ${context}, got ${value.tag}`)
  }
  if (value.caseIndex === URL_CASE) {
    const payload = value.payload
    if (!payload || payload.tag !== "url") {
      throw new Error(`Expected url payload for ${context}`)
    }
    return { _tag: "url", val: payload.value }
  }
  if (value.caseIndex === INLINE_CASE) {
    const payload = value.payload
    if (!payload || payload.tag !== "text") {
      throw new Error(`Expected inline text payload for ${context}`)
    }
    if (allowedCodes.length > 0 && payload.language && !allowedCodes.includes(payload.language)) {
      throw new Error(
        `Invalid value for ${context}. Language code \`${payload.language}\` is not allowed. Allowed codes: ${allowedCodes.join(", ")}`,
      )
    }
    return payload.language
      ? { _tag: "inline", val: payload.text, languageCode: payload.language }
      : { _tag: "inline", val: payload.text }
  }
  throw new Error(`Unknown unstructured-text variant case ${value.caseIndex} for ${context}`)
}

/** Encode a `BinaryReferenceValue` into the `variant { inline, url }` value. */
export const unstructuredBinaryToValue = (value: BinaryReferenceValue): SchemaValue => {
  if (value._tag === "url") {
    return v.variant(URL_CASE, { tag: "url", value: value.val })
  }
  return v.variant(INLINE_CASE, { tag: "binary", bytes: value.val, mimeType: value.mimeType })
}

/**
 * Decode a `variant { inline, url }` value into a `BinaryReferenceValue`.
 * Lenient: a missing mime type is always allowed; only a *present* mime type
 * outside a non-empty allow-list is rejected.
 */
export const unstructuredBinaryFromValue = (
  context: string,
  value: SchemaValue,
  allowedMimeTypes: ReadonlyArray<string>,
): BinaryReferenceValue => {
  if (value.tag !== "variant") {
    throw new Error(`Expected variant value for ${context}, got ${value.tag}`)
  }
  if (value.caseIndex === URL_CASE) {
    const payload = value.payload
    if (!payload || payload.tag !== "url") {
      throw new Error(`Expected url payload for ${context}`)
    }
    return { _tag: "url", val: payload.value }
  }
  if (value.caseIndex === INLINE_CASE) {
    const payload = value.payload
    if (!payload || payload.tag !== "binary") {
      throw new Error(`Expected inline binary payload for ${context}`)
    }
    if (
      allowedMimeTypes.length > 0 &&
      payload.mimeType &&
      !allowedMimeTypes.includes(payload.mimeType)
    ) {
      throw new Error(
        `Invalid value for ${context}. Mime type \`${payload.mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(", ")}`,
      )
    }
    return payload.mimeType
      ? { _tag: "inline", val: payload.bytes, mimeType: payload.mimeType }
      : { _tag: "inline", val: payload.bytes }
  }
  throw new Error(`Unknown unstructured-binary variant case ${value.caseIndex} for ${context}`)
}

// ---------- WitCodec assembly ----------

/**
 * Build a self-contained `WitCodec` from a variant `SchemaType` root and a
 * pure `{ toValue, fromValue }` pair, mirroring `toWitCodec`'s assembly.
 *
 * @since 1.5.0
 * @category codecs
 */
export const makeElementWitCodec = <T>(
  root: SchemaType,
  toValue: (d: T) => SchemaValue,
  fromValue: (sv: SchemaValue) => T,
): WitCodec<Schema.Codec<T, T>> => {
  const SchemaValueCarrier = Schema.declare((_u): _u is SchemaValue => true)
  const DomainCarrier = Schema.declare((_u): _u is T => true)
  const codec = SchemaValueCarrier.pipe(
    Schema.decodeTo(DomainCarrier, {
      decode: tryGetter((sv: SchemaValue) => fromValue(sv)),
      encode: tryGetter((d: T) => toValue(d)),
    }),
  ) as WitCodec<Schema.Codec<T, T>>["codec"]
  return {
    schema: DomainCarrier,
    graph: { defs: new Map(), root },
    isUnit: false,
    codec,
  }
}

/**
 * A `SchemaGetter.transformOrFail` that catches any error the pure transform
 * throws and surfaces it as a `SchemaIssue.InvalidValue` (so it lands on the
 * schema-error channel rather than escaping as an uncaught defect).
 *
 * @since 1.5.0
 * @category codecs
 */
export const tryGetter = <I, O>(f: (input: I) => O) =>
  SchemaGetter.transformOrFail((input: I) => {
    try {
      return Effect.succeed(f(input))
    } catch (e) {
      return Effect.fail(
        new SchemaIssue.InvalidValue(Option.some(input), {
          message: e instanceof Error ? e.message : String(e),
        }),
      )
    }
  })

// ---------- Boundary carrier ----------

/**
 * Tagged carrier returned by `UnstructuredText()` / `UnstructuredBinary()`.
 *
 * It holds a pre-built {@link WitCodec} (so the method / agent param compiler
 * can route it straight into the shared schema-graph) plus the raw variant
 * `root` schema type and a value codec — both reused by {@link multimodal}
 * when this element appears as a multimodal case.
 *
 * Intentionally NOT a `Schema.Top`: unstructured elements project to dedicated
 * `text` / `binary` schema nodes carrying a role, not arbitrary structural
 * schemas, so allowing them inside `Schema.Struct` etc. would be a category
 * error.
 *
 * @since 1.5.0
 * @category models
 */
export interface ElementSpec<T> {
  readonly _effectGolem: "ElementSpec"
  /** Pre-built WitCodec routing this element through the schema-graph. */
  readonly witCodec: WitCodec<Schema.Codec<T, T>>
  /** The variant `SchemaType` root (reused as a multimodal case). */
  readonly root: SchemaType
  /** Encode a domain value into the variant `SchemaValue`. */
  readonly toValue: (value: T) => SchemaValue
  /** Decode a variant `SchemaValue` back into a domain value. */
  readonly fromValue: (sv: SchemaValue) => T
}

/**
 * Type-guard for `ElementSpec` carriers.
 *
 * @since 1.5.0
 * @category guards
 */
export const isElementSpec = (x: unknown): x is ElementSpec<unknown> =>
  typeof x === "object" &&
  x !== null &&
  (x as { _effectGolem?: unknown })._effectGolem === "ElementSpec"

// ---------- Public factories ----------

/**
 * Restriction descriptor accepted by `UnstructuredText()`.
 *
 * @since 1.5.0
 * @category models
 */
export interface TextRestriction {
  readonly languageCode: string
}

/**
 * Restriction descriptor accepted by `UnstructuredBinary()`.
 *
 * @since 1.5.0
 * @category models
 */
export interface BinaryRestriction {
  readonly mimeType: string
}

/**
 * Element spec for an unstructured text input parameter.
 *
 * The parameter value at the user side is a {@link TextReferenceValue} (`url`
 * or `inline`). Restrictions, if provided, surface in the emitted `text`
 * schema node's `restrictions.languages`.
 *
 * @since 1.5.0
 * @category constructors
 */
export const UnstructuredText = (opts?: {
  readonly restrictions?: ReadonlyArray<TextRestriction>
}): ElementSpec<TextReferenceValue> => {
  const languages = opts?.restrictions?.map((r) => r.languageCode) ?? []
  const root = unstructuredTextSchemaType(languages)
  const toValue = unstructuredTextToValue
  const fromValue = (sv: SchemaValue): TextReferenceValue =>
    unstructuredTextFromValue("UnstructuredText element", sv, languages)
  return {
    _effectGolem: "ElementSpec",
    witCodec: makeElementWitCodec(root, toValue, fromValue),
    root,
    toValue,
    fromValue,
  }
}

/**
 * Element spec for an unstructured binary input parameter.
 *
 * @since 1.5.0
 * @category constructors
 */
export const UnstructuredBinary = (opts?: {
  readonly restrictions?: ReadonlyArray<BinaryRestriction>
}): ElementSpec<BinaryReferenceValue> => {
  const mimeTypes = opts?.restrictions?.map((r) => r.mimeType) ?? []
  const root = unstructuredBinarySchemaType(mimeTypes)
  const toValue = unstructuredBinaryToValue
  const fromValue = (sv: SchemaValue): BinaryReferenceValue =>
    unstructuredBinaryFromValue("UnstructuredBinary element", sv, mimeTypes)
  return {
    _effectGolem: "ElementSpec",
    witCodec: makeElementWitCodec(root, toValue, fromValue),
    root,
    toValue,
    fromValue,
  }
}
