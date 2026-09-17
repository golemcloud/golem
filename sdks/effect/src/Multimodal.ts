/**
 * @since 1.5.0
 */
import { Effect, Schema, SchemaGetter } from "effect"
import type { Role } from "golem:core/types@2.0.0"
import { composeSchemaGraphs } from "./internal/schema-model/builder.js"
import {
  emptyMetadata,
  t,
  v,
  variantCase,
  type SchemaType,
  type SchemaValue,
  type VariantCaseType,
} from "./internal/schema-model/model.js"
import {
  isElementSpec,
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type ElementSpec,
  type TextReferenceValue,
} from "./Unstructured.js"
import { toWitCodec, type UnsupportedSchemaError, type WitCodec } from "./WitCodec.js"

const MULTIMODAL_ROLE: Role = { tag: "multimodal" }

/**
 * One named element of a multimodal payload — either an
 * {@link ElementSpec} (unstructured-text/binary) or a regular
 * `Schema.Top` (compiled via {@link toWitCodec}).
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

type DecodingServices<S extends MultimodalShape> = S[keyof S] extends infer M
  ? M extends Schema.Top
    ? M["DecodingServices"]
    : never
  : never

type EncodingServices<S extends MultimodalShape> = S[keyof S] extends infer M
  ? M extends Schema.Top
    ? M["EncodingServices"]
    : never
  : never

type MultimodalSchema<S extends MultimodalShape> = Schema.Codec<
  MultimodalValue<S>,
  MultimodalValue<S>,
  DecodingServices<S>,
  EncodingServices<S>
>

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
 * A `Multimodal<S>` is the carrier produced by {@link multimodal}. It lives at
 * the same boundary layer as {@link ElementSpec}: not a `Schema.Top`, but
 * recognised by the method / agent compiler as a parameter that projects to a
 * `list<variant>` schema node tagged `role = multimodal`.
 *
 * It holds a pre-built {@link WitCodec} whose root is that `list<variant>`. The
 * value codec maps the domain array (`ReadonlyArray<{ _tag, value }>`) to/from
 * a `{ tag: "list", elements: [v.variant(caseIndex, payload), …] }` value.
 *
 * @since 1.5.0
 * @category models
 */
export interface Multimodal<S extends MultimodalShape> {
  readonly _effectGolem: "Multimodal"
  readonly shape: S
  /**
   * Compile this multimodal carrier to its `WitCodec`. Done lazily / cached so
   * the method & agent param compilers share one assembly.
   */
  readonly compile: () => Effect.Effect<WitCodec<MultimodalSchema<S>>, UnsupportedSchemaError>
}

/** Per-case bridge between a domain value and its `SchemaValue`. */
interface CaseCodec<RD = never, RE = never> {
  readonly name: string
  readonly graph: WitCodec<Schema.Top>["graph"]
  readonly toValue: (domain: unknown) => Effect.Effect<SchemaValue, Schema.SchemaError, RE>
  readonly fromValue: (sv: SchemaValue) => Effect.Effect<unknown, Schema.SchemaError, RD>
}

const compileMember = <M extends MultimodalMember>(
  caseName: string,
  member: M,
): Effect.Effect<
  CaseCodec<
    M extends Schema.Top ? M["DecodingServices"] : never,
    M extends Schema.Top ? M["EncodingServices"] : never
  >,
  UnsupportedSchemaError
> =>
  Effect.gen(function* () {
    if (isElementSpec(member)) {
      return {
        name: caseName,
        graph: member.witCodec.graph,
        toValue: (domain: unknown) => Effect.sync(() => member.toValue(domain)),
        fromValue: (sv: SchemaValue) => Effect.sync(() => member.fromValue(sv)),
      }
    }
    const wc = yield* toWitCodec(member)
    return {
      name: caseName,
      graph: wc.graph,
      toValue: (domain) => Schema.encodeEffect(wc.codec)(domain),
      fromValue: (sv) => Schema.decodeEffect(wc.codec)(sv),
    }
  }) as Effect.Effect<
    CaseCodec<
      M extends Schema.Top ? M["DecodingServices"] : never,
      M extends Schema.Top ? M["EncodingServices"] : never
    >,
    UnsupportedSchemaError
  >

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
 * ```
 *
 * @since 1.5.0
 * @category constructors
 */
export const multimodal = <S extends MultimodalShape>(shape: S): Multimodal<S> => {
  let cached: WitCodec<MultimodalSchema<S>> | null = null

  return {
    _effectGolem: "Multimodal",
    shape,
    compile: () =>
      Effect.suspend(() => {
        if (cached !== null) return Effect.succeed(cached)
        return Effect.gen(function* () {
          const cases: Array<CaseCodec<DecodingServices<S>, EncodingServices<S>>> = []
          for (const [k, m] of Object.entries(shape)) {
            cases.push(
              (yield* compileMember(k, m)) as CaseCodec<DecodingServices<S>, EncodingServices<S>>,
            )
          }
          const byName = new Map(cases.map((c, i) => [c.name, { c, index: i }] as const))

          const composed = composeSchemaGraphs(cases.map((member) => member.graph))
          const variantCases: Array<VariantCaseType> = cases.map((c, index) =>
            variantCase(c.name, composed.roots[index]!),
          )
          const variant = t.variant(variantCases)
          const root: SchemaType = {
            body: t.list(variant).body,
            metadata: { ...emptyMetadata(), role: MULTIMODAL_ROLE },
          }

          const toValue = (value: MultimodalValue<S>) =>
            Effect.gen(function* () {
              const elements: Array<SchemaValue> = []
              for (const item of value) {
                const entry = byName.get(item._tag)
                if (entry === undefined) {
                  throw new Error(`multimodal: unknown case '${item._tag}'`)
                }
                elements.push(v.variant(entry.index, yield* entry.c.toValue(item.value)))
              }
              return v.list(elements)
            })

          const fromValue = (sv: SchemaValue) =>
            Effect.gen(function* () {
              if (sv.tag !== "list") {
                throw new Error(`multimodal: expected a list value, got ${sv.tag}`)
              }
              const out: Array<{ _tag: string; value: unknown }> = []
              for (const el of sv.elements) {
                if (el.tag !== "variant") {
                  throw new Error(`multimodal: expected variant element, got ${el.tag}`)
                }
                const c = cases[el.caseIndex]
                if (c === undefined) {
                  throw new Error(`multimodal: unknown case index ${el.caseIndex}`)
                }
                if (el.payload === undefined) {
                  throw new Error(`multimodal: missing payload for case '${c.name}'`)
                }
                out.push({ _tag: c.name, value: yield* c.fromValue(el.payload) })
              }
              return out as unknown as MultimodalValue<S>
            })

          const SchemaValueCarrier = Schema.declare((_u): _u is SchemaValue => true)
          const DomainCarrier = Schema.declare((_u): _u is MultimodalValue<S> => true)
          const codec = SchemaValueCarrier.pipe(
            Schema.decodeTo(DomainCarrier, {
              decode: SchemaGetter.transformOrFail((sv: SchemaValue) =>
                fromValue(sv).pipe(Effect.mapError((error) => error.issue)),
              ),
              encode: SchemaGetter.transformOrFail((d: MultimodalValue<S>) =>
                toValue(d).pipe(Effect.mapError((error) => error.issue)),
              ),
            }),
          ) as WitCodec<MultimodalSchema<S>>["codec"]

          cached = {
            schema: DomainCarrier as MultimodalSchema<S>,
            graph: { defs: composed.defs, root },
            isUnit: false,
            codec,
          }
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
 * schema under a named slot (default: `"custom"`).
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
