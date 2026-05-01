/**
 * @since 0.1.0
 */
import { Effect, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import { ElementValueKindError, type ElementCodec } from "./element.js"
import { Uint8ArraySchema } from "./wit-types.js"

// ---------- Wire-shape schemas (composable in component-model records) ----------

/**
 * Schema for `golem:core/types@1.5.0`.TextType.
 *
 * @since 0.1.0
 * @category codecs
 */
export const TextType = Schema.Struct({ languageCode: Schema.String })

/**
 * Schema for `golem:core/types@1.5.0`.BinaryType.
 *
 * @since 0.1.0
 * @category codecs
 */
export const BinaryType = Schema.Struct({ mimeType: Schema.String })

/**
 * Schema for `golem:core/types@1.5.0`.TextSource.
 *
 * @since 0.1.0
 * @category codecs
 */
export const TextSource = Schema.Struct({
  data: Schema.String,
  textType: Schema.optionalKey(TextType),
})

/**
 * Schema for `golem:core/types@1.5.0`.BinarySource.
 *
 * @since 0.1.0
 * @category codecs
 */
export const BinarySource = Schema.Struct({
  data: Uint8ArraySchema,
  binaryType: BinaryType,
})

/**
 * Schema for `golem:core/types@1.5.0`.TextReference — `url` (a plain
 * string URL) or `inline` (a `TextSource` carrying the data and an
 * optional `TextType` hint).
 *
 * @since 0.1.0
 * @category codecs
 */
export const TextReference = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("url"), val: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("inline"), val: TextSource }),
])

/**
 * Schema for `golem:core/types@1.5.0`.BinaryReference — `url` or `inline`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const BinaryReference = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("url"), val: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("inline"), val: BinarySource }),
])

// ---------- Boundary types ----------

/**
 * Domain-side shape of a `TextReference` (`Schema.TaggedUnion`-style).
 * The wire shape is the same — `Schema.Union` returns the same encoded
 * JS shape.
 *
 * @since 0.1.0
 * @category models
 */
export type TextReferenceValue = typeof TextReference.Type

/**
 * Domain-side shape of a `BinaryReference`.
 *
 * @since 0.1.0
 * @category models
 */
export type BinaryReferenceValue = typeof BinaryReference.Type

/**
 * Tagged carrier returned by `UnstructuredText()` / `UnstructuredBinary()`.
 *
 * `compileMethodSpec` recognises this carrier in a method's `params`
 * record and emits an `unstructured-*` `ElementSchema` instead of
 * routing the parameter through `toWitCodec`. Inside the user's handler
 * the parameter is typed as the corresponding reference value.
 *
 * This is intentionally NOT a `Schema.Top`: unstructured elements live
 * at the data-element layer of the WIT model, not the component-model
 * layer, so allowing them inside `Schema.Struct` etc. would be a
 * category error. Embed `TextReference` / `BinaryReference` schemas
 * instead when you need that.
 *
 * @since 0.1.0
 * @category models
 */
export interface ElementSpec<T> {
  readonly _effectGolem: "ElementSpec"
  readonly element: ElementCodec<T>
}

/**
 * Type-guard for `ElementSpec` carriers.
 *
 * @since 0.1.0
 * @category guards
 */
export const isElementSpec = (x: unknown): x is ElementSpec<unknown> =>
  typeof x === "object" &&
  x !== null &&
  (x as { _effectGolem?: unknown })._effectGolem === "ElementSpec"

const passthroughReference = <V>(
  expected: AgentCommon.ElementSchema["tag"],
  context: string,
): {
  encode: (v: V) => Effect.Effect<CoreTypes.ElementValue, Schema.SchemaError>
  decode: (
    e: CoreTypes.ElementValue,
  ) => Effect.Effect<V, Schema.SchemaError | ElementValueKindError>
} => ({
  encode: (v) => Effect.succeed({ tag: expected, val: v as any } as CoreTypes.ElementValue),
  decode: (element) => {
    if (element.tag !== expected) {
      return Effect.fail(new ElementValueKindError(expected, element.tag, context))
    }
    return Effect.succeed((element as unknown as { val: V }).val)
  },
})

// ---------- Public factories ----------

/**
 * Restriction descriptor accepted by `UnstructuredText()`.
 *
 * @since 0.1.0
 * @category models
 */
export interface TextRestriction {
  readonly languageCode: string
}

/**
 * Restriction descriptor accepted by `UnstructuredBinary()`.
 *
 * @since 0.1.0
 * @category models
 */
export interface BinaryRestriction {
  readonly mimeType: string
}

/**
 * Element spec for an unstructured text input parameter.
 *
 * The parameter value at the user side is a `TextReferenceValue` (`url`
 * or `inline`). Restrictions, if provided, surface in the emitted
 * `ElementSchema`'s `restrictions` field.
 *
 * @since 0.1.0
 * @category constructors
 */
export const UnstructuredText = (opts?: {
  readonly restrictions?: ReadonlyArray<TextRestriction>
}): ElementSpec<TextReferenceValue> => {
  const elementSchema: AgentCommon.ElementSchema = {
    tag: "unstructured-text",
    val: {
      restrictions: opts?.restrictions
        ? opts.restrictions.map((r) => ({ languageCode: r.languageCode }))
        : undefined,
    },
  }
  // Reference values match the wire shape exactly, so we pass them
  // through verbatim.
  const io = passthroughReference<TextReferenceValue>(
    "unstructured-text",
    "UnstructuredText element",
  )
  return {
    _effectGolem: "ElementSpec",
    element: { elementSchema, ...io },
  }
}

/**
 * Element spec for an unstructured binary input parameter.
 *
 * @since 0.1.0
 * @category constructors
 */
export const UnstructuredBinary = (opts?: {
  readonly restrictions?: ReadonlyArray<BinaryRestriction>
}): ElementSpec<BinaryReferenceValue> => {
  const elementSchema: AgentCommon.ElementSchema = {
    tag: "unstructured-binary",
    val: {
      restrictions: opts?.restrictions
        ? opts.restrictions.map((r) => ({ mimeType: r.mimeType }))
        : undefined,
    },
  }
  const io = passthroughReference<BinaryReferenceValue>(
    "unstructured-binary",
    "UnstructuredBinary element",
  )
  return {
    _effectGolem: "ElementSpec",
    element: { elementSchema, ...io },
  }
}
