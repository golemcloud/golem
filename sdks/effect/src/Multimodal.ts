/**
 * @since 1.5.0
 */
import { Effect, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import { componentModelElement, ElementValueKindError, type ElementCodec } from "./Element.js"
import {
  isElementSpec,
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type ElementSpec,
  type TextReferenceValue,
} from "./Unstructured.js"
import { toWitCodec, type UnsupportedSchemaError } from "./WitCodec.js"

/**
 * One named element of a multimodal payload — either an
 * {@link ElementSpec} (unstructured-text/binary) or a regular
 * `Schema.Top` (compiled to a `component-model` element).
 *
 * @since 1.5.0
 * @category models
 */
export type MultimodalMember = ElementSpec<any> | Schema.Top

/**
 * Record mapping case names to their multimodal members.
 *
 * @since 1.5.0
 * @category models
 */
export type MultimodalShape = Readonly<Record<string, MultimodalMember>>

/**
 * Domain-side type emitted by a multimodal element of the given shape.
 *
 * @since 1.5.0
 * @category models
 */
export type MultimodalValue<S extends MultimodalShape> = ReadonlyArray<
  {
    readonly [K in keyof S & string]: {
      readonly _tag: K
      readonly value: S[K] extends ElementSpec<infer T>
        ? T
        : S[K] extends Schema.Top
          ? S[K]["Type"]
          : never
    }
  }[keyof S & string]
>

/**
 * A `Multimodal<S>` is the carrier produced by {@link multimodal}. It
 * lives at the same boundary layer as `ElementSpec`: not a `Schema.Top`,
 * but recognised by the method/agent compiler as a special parameter
 * that maps to a `DataSchema { tag: "multimodal", val: ... }` slot.
 *
 * The carrier captures everything needed to encode/decode multimodal
 * `DataValue.multimodal` payloads to/from a `ReadonlyArray<{_tag, value}>`.
 *
 * @since 1.5.0
 * @category models
 */
export interface Multimodal<S extends MultimodalShape> {
  readonly _effectGolem: "Multimodal"
  readonly shape: S
  /**
   * Compile this multimodal carrier to per-case `ElementCodec`s and the
   * matching `DataSchema.multimodal` payload. Done lazily by
   * `compileMethodSpec` / `compileParamBindings`.
   */
  readonly compile: () => Effect.Effect<
    {
      readonly dataSchema: AgentCommon.DataSchema
      readonly encode: (
        value: MultimodalValue<S>,
      ) => Effect.Effect<CoreTypes.DataValue, Schema.SchemaError>
      readonly decode: (
        dv: CoreTypes.DataValue,
      ) => Effect.Effect<MultimodalValue<S>, Schema.SchemaError | ElementValueKindError>
    },
    UnsupportedSchemaError
  >
}

const compileMember = (
  caseName: string,
  member: MultimodalMember,
): Effect.Effect<ElementCodec<unknown>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    if (isElementSpec(member)) {
      return member.element as ElementCodec<unknown>
    }
    const wc = yield* toWitCodec(member)
    return componentModelElement(wc, `multimodal[${caseName}]`) as ElementCodec<unknown>
  })

/**
 * Construct a multimodal element that accepts a named, ordered, repeatable
 * sequence of typed sub-elements.
 *
 * **Example**
 *
 * ```ts
 * const Content = multimodal({
 *   text:  UnstructuredText(),
 *   image: UnstructuredBinary(),
 *   meta:  Schema.Struct({ prompt: Schema.String }),
 * })
 *
 * defineAgent({
 *   ...,
 *   methods: {
 *     send: method({
 *       params: { content: Content },         // single multimodal param
 *       success: Schema.String,
 *     }),
 *   },
 *   impl: () => Effect.succeed({
 *     send: ({ content }) =>
 *       // content: ReadonlyArray<
 *       //   | { _tag: "text",  value: TextReference }
 *       //   | { _tag: "image", value: BinaryReference }
 *       //   | { _tag: "meta",  value: { prompt: string } }
 *       // >
 *       Effect.succeed("ok"),
 *   }),
 * })
 * ```
 *
 * @since 1.5.0
 * @category constructors
 */
export const multimodal = <S extends MultimodalShape>(shape: S): Multimodal<S> => {
  let cached: {
    readonly dataSchema: AgentCommon.DataSchema
    readonly encode: (
      value: MultimodalValue<S>,
    ) => Effect.Effect<CoreTypes.DataValue, Schema.SchemaError>
    readonly decode: (
      dv: CoreTypes.DataValue,
    ) => Effect.Effect<MultimodalValue<S>, Schema.SchemaError | ElementValueKindError>
  } | null = null

  return {
    _effectGolem: "Multimodal",
    shape,
    compile: () =>
      Effect.suspend(() => {
        if (cached !== null) return Effect.succeed(cached)
        return Effect.gen(function* () {
          const entries: Array<[string, ElementCodec<unknown>]> = []
          for (const [k, m] of Object.entries(shape)) {
            const ec = yield* compileMember(k, m)
            entries.push([k, ec])
          }
          const byName = new Map(entries)
          const dataSchema: AgentCommon.DataSchema = {
            tag: "multimodal",
            val: entries.map(([k, ec]) => [k, ec.elementSchema]),
          }
          const encode = (
            value: MultimodalValue<S>,
          ): Effect.Effect<CoreTypes.DataValue, Schema.SchemaError> =>
            Effect.gen(function* () {
              const out: Array<[string, CoreTypes.ElementValue]> = []
              for (const item of value) {
                const ec = byName.get(item._tag)
                if (ec === undefined) {
                  // Unknown _tag: encode as a SchemaError-shaped failure
                  // by routing through Effect.die so the caller sees a
                  // clear runtime mismatch.
                  return yield* Effect.die(new Error(`multimodal: unknown case '${item._tag}'`))
                }
                const ev = yield* ec.encode(item.value)
                out.push([item._tag, ev])
              }
              return { tag: "multimodal", val: out } as CoreTypes.DataValue
            })
          const decode = (
            dv: CoreTypes.DataValue,
          ): Effect.Effect<MultimodalValue<S>, Schema.SchemaError | ElementValueKindError> =>
            Effect.gen(function* () {
              if (dv.tag !== "multimodal") {
                return yield* Effect.fail(
                  new ElementValueKindError(
                    "unstructured-text",
                    dv.tag as CoreTypes.ElementValue["tag"],
                    "multimodal: expected DataValue.multimodal",
                  ),
                )
              }
              const out: Array<{ _tag: string; value: unknown }> = []
              for (const [name, ev] of dv.val) {
                const ec = byName.get(name)
                if (ec === undefined) {
                  return yield* Effect.die(new Error(`multimodal: unknown case '${name}'`))
                }
                const v = yield* ec.decode(ev)
                out.push({ _tag: name, value: v })
              }
              return out as unknown as MultimodalValue<S>
            })
          cached = { dataSchema, encode, decode }
          return cached
        })
      }),
  }
}

/**
 * Type-guard for `Multimodal` carriers.
 *
 * @since 1.5.0
 * @category guards
 */
export const isMultimodal = (x: unknown): x is Multimodal<MultimodalShape> =>
  typeof x === "object" &&
  x !== null &&
  (x as { _effectGolem?: unknown })._effectGolem === "Multimodal"

// ---------- Convenience constructors ----------

/**
 * Multimodal payload with arbitrary text + image elements.
 *
 * @since 1.5.0
 * @category constructors
 */
export const multimodalTextImage = (opts?: {
  readonly text?: { readonly restrictions?: ReadonlyArray<{ languageCode: string }> }
  readonly image?: { readonly restrictions?: ReadonlyArray<{ mimeType: string }> }
}) =>
  multimodal({
    text: UnstructuredText(opts?.text),
    image: UnstructuredBinary(opts?.image),
  } as const)

/**
 * Multimodal payload with arbitrary text + image elements plus a custom
 * component-model schema under a named slot (default: `"custom"`).
 *
 * @since 1.5.0
 * @category constructors
 */
export const multimodalTextImageCustom = <S extends Schema.Top>(
  custom: S,
  opts?: {
    readonly text?: { readonly restrictions?: ReadonlyArray<{ languageCode: string }> }
    readonly image?: { readonly restrictions?: ReadonlyArray<{ mimeType: string }> }
    readonly customName?: string
  },
) => {
  const name = opts?.customName ?? "custom"
  const shape: Record<string, MultimodalMember> = {
    text: UnstructuredText(opts?.text),
    image: UnstructuredBinary(opts?.image),
    [name]: custom,
  }
  return multimodal(shape) as unknown as Multimodal<
    {
      text: ElementSpec<TextReferenceValue>
      image: ElementSpec<BinaryReferenceValue>
    } & Record<string, S>
  >
}
