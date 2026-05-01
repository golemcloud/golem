/**
 * @since 0.1.0
 */
import { Effect, HashMap, Option, Result, Schema, SchemaAST, SchemaGetter } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import { witGraphCodec, type WitValueTree } from "./WitTree.js"
import {
  variantCaseNameAnnotationKey,
  witTypeAnnotationKey,
  witTypedArrayAnnotationKey,
  type WitNumericKind,
  type WitTypedArrayKind,
} from "./WitTypes.js"

type WitTypeNode = CoreTypes.WitTypeNode
type NamedWitTypeNode = CoreTypes.NamedWitTypeNode
type WitType = CoreTypes.WitType
type WitValue = CoreTypes.WitValue

// Branded so `Durability.wrap` (and any other downstream consumer
// that uses nominal SDK-error detection) can route this into the
// defect channel without `_tag`-string sniffing. Declared as a
// `unique symbol` const because TypeScript requires class-property
// computed names to have either a literal type or a `unique symbol`;
// `Symbol.for(...)` guarantees the same runtime symbol across modules.
const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/**
 * Raised by {@link toWitCodec} (and by registration helpers that compile
 * a user schema to a `WitType`) when an Effect Schema construct cannot
 * be represented in the WIT type system.
 *
 * @since 0.1.0
 * @category errors
 */
export class UnsupportedSchemaError {
  readonly _tag = "UnsupportedSchemaError"
  readonly [sdkErrorBrand] = true
  constructor(readonly reason: string) {}
}

/**
 * A pair of pure tree transforms that mirrors a single AST node onto its
 * `WitValueTree` representation. They operate on the user schema's
 * **encoded** values, never the decoded domain values — that way the
 * user's own decode/encode logic still runs (refinements, transformations,
 * branded types, …) when the full codec is evaluated.
 */
interface TransformPair {
  /** Encoded → tree. Called during `Schema.encode`. */
  readonly toTree: (encoded: any) => WitValueTree
  /** Tree → encoded. Called during `Schema.decode`. */
  readonly fromTree: (tree: WitValueTree) => any
}

/**
 * The full mapping for a single Effect Schema:
 *
 * - `witType`         — a Golem `WitType` graph (root-first, indices remapped)
 * - `elementSchema`   — the same wrapped as an `ElementSchema`
 * - `codec`           — `Codec<domainType, WitValue>`, composed via
 *                       Effect-Schema combinators on top of the user's own
 *                       schema, so refinements/transformations are honoured
 *
 * @since 0.1.0
 * @category codecs
 */
export interface WitCodec<S extends Schema.Top> {
  readonly schema: S
  readonly witType: WitType
  readonly elementSchema: AgentCommon.ElementSchema
  readonly codec: Schema.Codec<S["Type"], WitValue, S["DecodingServices"], S["EncodingServices"]>
}

const leafPair = (tag: WitValueTree["tag"]): TransformPair => ({
  toTree: (v) => ({ tag, val: v }) as WitValueTree,
  fromTree: (t) => (t as { val: unknown }).val,
})

/**
 * Mapping from a {@link WitNumericKind} annotation to the matching WIT
 * type-node tag, value-node tag (used by the `TransformPair`), and a
 * coercion that re-shapes the encoded JS value into whatever the value
 * tree expects (e.g. `bigint` for `prim-u64`).
 */
const numericMapping: Record<
  WitNumericKind,
  { typeTag: WitTypeNode["tag"]; valueTag: WitValueTree["tag"]; coerce: (v: any) => any }
> = {
  u8: { typeTag: "prim-u8-type", valueTag: "prim-u8", coerce: (v) => v },
  u16: { typeTag: "prim-u16-type", valueTag: "prim-u16", coerce: (v) => v },
  u32: { typeTag: "prim-u32-type", valueTag: "prim-u32", coerce: (v) => v },
  u64: {
    typeTag: "prim-u64-type",
    valueTag: "prim-u64",
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  s8: { typeTag: "prim-s8-type", valueTag: "prim-s8", coerce: (v) => v },
  s16: { typeTag: "prim-s16-type", valueTag: "prim-s16", coerce: (v) => v },
  s32: { typeTag: "prim-s32-type", valueTag: "prim-s32", coerce: (v) => v },
  s64: {
    typeTag: "prim-s64-type",
    valueTag: "prim-s64",
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  f32: { typeTag: "prim-float32-type" as any, valueTag: "prim-float32", coerce: (v) => v },
  f64: { typeTag: "prim-f64-type", valueTag: "prim-float64", coerce: (v) => v },
}
// f32 type tag is actually `prim-f32-type`, fix the entry above.
numericMapping.f32.typeTag = "prim-f32-type"

/**
 * Look up an annotation by key on an AST node. Effect Schema attaches
 * annotations from `Schema.annotate(...)` to the *last check* (refinement),
 * not the AST root, so we delegate to `SchemaAST.resolveAt` which knows
 * the right traversal order.
 */
const annotationOf = <T = unknown>(a: SchemaAST.AST, key: string): T | undefined =>
  (
    SchemaAST as unknown as {
      resolveAt: <U>(k: string) => (a: SchemaAST.AST) => U | undefined
    }
  ).resolveAt<T>(key)(a)

const numericKindOf = (a: SchemaAST.AST): WitNumericKind | undefined =>
  annotationOf<WitNumericKind>(a, witTypeAnnotationKey)

const numericPair = (kind: WitNumericKind): TransformPair => {
  const m = numericMapping[kind]
  return {
    toTree: (v) => ({ tag: m.valueTag, val: m.coerce(v) }) as WitValueTree,
    fromTree: (t) => (t as { val: unknown }).val,
  }
}

const numericNode = (kind: WitNumericKind): { node: WitTypeNode; pair: TransformPair } => ({
  node: { tag: numericMapping[kind].typeTag } as WitTypeNode,
  pair: numericPair(kind),
})

const isNullOrUndefinedAST = (a: SchemaAST.AST): boolean =>
  a._tag === "Null" || a._tag === "Undefined" || a._tag === "Void"

/**
 * A "shape signature" classifying an AST's *encoded* form, used to:
 *
 * - dispatch encoded values to the matching variant case at encode time
 *   (`matches(encoded)` is a runtime predicate over the encoded value)
 * - reject unions whose members would be ambiguous at encode time
 *   (`tag` collisions in non-`object`/`array` shapes)
 */
interface EncodedShape {
  readonly tag:
    | "string"
    | "number"
    | "boolean"
    | "bigint"
    | "null"
    | "undefined"
    | "literal"
    | "array"
    | "object"
    | "object-with-tag"
    | "unknown"
  readonly literal?: string | number | boolean | bigint
  /** For `object-with-tag`: the discriminator value of the `_tag` field. */
  readonly tagLiteral?: string
  readonly matches: (v: unknown) => boolean
}

/**
 * Compute an `EncodedShape` for a schema AST. Operates on the *encoded*
 * form because the codec walks encoded values, not decoded ones — so any
 * downstream `decodeTo`/refinement is irrelevant here.
 */
const encodedShapeOf = (a: SchemaAST.AST): EncodedShape => {
  switch (a._tag) {
    case "String":
      return { tag: "string", matches: (v) => typeof v === "string" }
    case "Number":
      return { tag: "number", matches: (v) => typeof v === "number" }
    case "Boolean":
      return { tag: "boolean", matches: (v) => typeof v === "boolean" }
    case "BigInt":
      return { tag: "bigint", matches: (v) => typeof v === "bigint" }
    case "Null":
      return { tag: "null", matches: (v) => v === null }
    case "Undefined":
    case "Void":
      return { tag: "undefined", matches: (v) => v === undefined }
    case "Literal": {
      const lit = (a as SchemaAST.Literal).literal
      return {
        tag: "literal",
        literal: lit as EncodedShape["literal"],
        matches: (v) => v === lit,
      }
    }
    case "Arrays":
      return { tag: "array", matches: Array.isArray }
    case "Objects": {
      const tagPs = (a as SchemaAST.Objects).propertySignatures.find((ps) => ps.name === "_tag")
      // Only treat _tag as a discriminator if the property is *required*
      // (a missing _tag must never produce a "matched" value).
      if (
        tagPs !== undefined &&
        !SchemaAST.isOptional(tagPs.type) &&
        SchemaAST.isLiteral(tagPs.type) &&
        typeof tagPs.type.literal === "string"
      ) {
        const lit = tagPs.type.literal as string
        return {
          tag: "object-with-tag",
          tagLiteral: lit,
          matches: (v) =>
            typeof v === "object" &&
            v !== null &&
            !Array.isArray(v) &&
            (v as { _tag?: unknown })._tag === lit,
        }
      }
      return {
        tag: "object",
        matches: (v) => typeof v === "object" && v !== null && !Array.isArray(v),
      }
    }
    default:
      return { tag: "unknown", matches: () => true }
  }
}

const variantCaseNameOf = (a: SchemaAST.AST): string | undefined =>
  annotationOf<string>(a, variantCaseNameAnnotationKey)

const declarationConstructorTag = (a: SchemaAST.AST): string | undefined => {
  if (a._tag !== "Declaration") return undefined
  const tc = (a.annotations as { typeConstructor?: { _tag?: string } } | undefined)?.typeConstructor
  return tc?._tag
}

const typedArrayKindOf = (a: SchemaAST.AST): WitTypedArrayKind | undefined =>
  annotationOf<WitTypedArrayKind>(a, witTypedArrayAnnotationKey)

/**
 * Per-typed-array element WIT primitive (type tag + value tag) plus an
 * optional element coercion (used to keep `bigint` payloads for s64/u64
 * arrays without forcing the user to pre-convert).
 */
const typedArrayElement: Record<
  WitTypedArrayKind,
  {
    typeTag: WitTypeNode["tag"]
    valueTag: WitValueTree["tag"]
    coerce: (v: unknown) => any
  }
> = {
  u8: { typeTag: "prim-u8-type", valueTag: "prim-u8", coerce: (v) => v },
  i8: { typeTag: "prim-s8-type", valueTag: "prim-s8", coerce: (v) => v },
  u16: { typeTag: "prim-u16-type", valueTag: "prim-u16", coerce: (v) => v },
  i16: { typeTag: "prim-s16-type", valueTag: "prim-s16", coerce: (v) => v },
  u32: { typeTag: "prim-u32-type", valueTag: "prim-u32", coerce: (v) => v },
  i32: { typeTag: "prim-s32-type", valueTag: "prim-s32", coerce: (v) => v },
  f32: { typeTag: "prim-f32-type", valueTag: "prim-float32", coerce: (v) => v },
  f64: { typeTag: "prim-f64-type", valueTag: "prim-float64", coerce: (v) => v },
  "big-i64": {
    typeTag: "prim-s64-type",
    valueTag: "prim-s64",
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  "big-u64": {
    typeTag: "prim-u64-type",
    valueTag: "prim-u64",
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
}

/** Construct the JS TypedArray subclass matching a `WitTypedArrayKind`. */
const typedArrayCtor: Record<WitTypedArrayKind, new (entries: Iterable<any>) => any> = {
  u8: Uint8Array,
  i8: Int8Array,
  u16: Uint16Array,
  i16: Int16Array,
  u32: Uint32Array,
  i32: Int32Array,
  f32: Float32Array,
  f64: Float64Array,
  "big-i64": BigInt64Array,
  "big-u64": BigUint64Array,
}

/**
 * Walk a Schema AST, producing a `WitType` graph and a top-level
 * `TransformPair`. Children are pushed depth-first; the root is reserved at
 * index 0 from the start so we never have to remap indices afterwards.
 */
const walk = (
  ast: SchemaAST.AST,
): Effect.Effect<{ witType: WitType; pair: TransformPair }, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    // Reserve slot 0 for the root. The placeholder is overwritten at the
    // end. All `push` calls go to indices >= 1.
    const nodes: Array<NamedWitTypeNode> = [{ type: { tag: "prim-bool-type" } }]

    const push = (type: WitTypeNode, name?: string): number => {
      const idx = nodes.length
      nodes.push(name === undefined ? { type } : { name, type })
      return idx
    }

    const unsupported = (reason: string) => Effect.fail(new UnsupportedSchemaError(reason))

    const nameOf = (a: SchemaAST.AST): string | undefined =>
      SchemaAST.resolveIdentifier(a) ?? SchemaAST.resolveTitle(a)

    /**
     * Build a child node, push it into `nodes`, and return both its index
     * and the transform-pair to use at the parent level. `nameOverride`
     * lets variant payloads keep their existing naming convention.
     */
    const child = (
      a: SchemaAST.AST,
      nameOverride?: string,
    ): Effect.Effect<{ idx: number; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const { node, pair } = yield* nodeFor(a)
        const idx = push(node, nameOverride ?? nameOf(a))
        return { idx, pair }
      })

    /**
     * Build a record-type node + pair given a list of property signatures.
     * Used for both `Objects` schemas and tagged-variant payloads.
     */
    const recordNode = (
      props: ReadonlyArray<SchemaAST.PropertySignature>,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        type Field = {
          readonly name: string
          readonly idx: number
          readonly pair: TransformPair
          readonly optional: boolean
        }
        const fields: Array<Field> = []
        for (const ps of props) {
          if (typeof ps.name !== "string") {
            return yield* unsupported(`non-string property key: ${String(ps.name)}`)
          }
          const { idx: rawIdx, pair: rawPair } = yield* child(ps.type)
          const optional = SchemaAST.isOptional(ps.type)
          if (optional) {
            const optIdx = push({ tag: "option-type", val: rawIdx })
            const pair: TransformPair = {
              toTree: (v) =>
                ({
                  tag: "option-value",
                  val: v === undefined ? undefined : rawPair.toTree(v),
                }) as WitValueTree,
              fromTree: (t) => {
                const ov = t as { val: WitValueTree | undefined }
                return ov.val === undefined ? undefined : rawPair.fromTree(ov.val)
              },
            }
            fields.push({ name: ps.name, idx: optIdx, pair, optional })
          } else {
            fields.push({ name: ps.name, idx: rawIdx, pair: rawPair, optional })
          }
        }
        const node: WitTypeNode = {
          tag: "record-type",
          val: fields.map((f) => [f.name, f.idx]),
        }
        const pair: TransformPair = {
          toTree: (obj: Record<string, unknown>) => ({
            tag: "record-value",
            val: fields.map((f) => f.pair.toTree(obj[f.name])),
          }),
          fromTree: (t) => {
            const rv = t as { val: ReadonlyArray<WitValueTree> }
            const out: Record<string, unknown> = {}
            for (let i = 0; i < fields.length; i++) {
              const f = fields[i]!
              const v = f.pair.fromTree(rv.val[i]!)
              if (!f.optional || v !== undefined) out[f.name] = v
            }
            return out
          },
        }
        return { node, pair }
      })

    /**
     * Build an option-type node + pair wrapping an inner index/pair, with
     * a configurable "encoded empty" representation (the value used to
     * stand in for `None` in the user's encoded form, e.g. `null` for
     * `NullOr`, `undefined` for `UndefinedOr`).
     */
    const optionWrap = (
      innerIdx: number,
      innerPair: TransformPair,
      empty: { readonly kind: "null" | "undefined" | "effect-option" },
    ): { node: WitTypeNode; pair: TransformPair } => {
      const isEmpty = (v: unknown): boolean => {
        switch (empty.kind) {
          case "null":
            return v === null
          case "undefined":
            return v === undefined
          case "effect-option":
            return Option.isOption(v as any) && Option.isNone(v as any)
        }
      }
      const wrap = (encoded: unknown): unknown => {
        switch (empty.kind) {
          case "null":
            return encoded
          case "undefined":
            return encoded
          case "effect-option":
            return Option.some(encoded)
        }
      }
      const unwrap = (v: any): unknown => {
        if (empty.kind === "effect-option") {
          return (v as Option.Option<unknown>).pipe(Option.getOrElse(() => undefined))
        }
        return v
      }
      const emptyEncoded = (): unknown => {
        switch (empty.kind) {
          case "null":
            return null
          case "undefined":
            return undefined
          case "effect-option":
            return Option.none()
        }
      }
      const node: WitTypeNode = { tag: "option-type", val: innerIdx }
      const pair: TransformPair = {
        toTree: (v) => ({
          tag: "option-value",
          val: isEmpty(v) ? undefined : innerPair.toTree(unwrap(v)),
        }),
        fromTree: (t) => {
          const ov = t as { val: WitValueTree | undefined }
          if (ov.val === undefined) return emptyEncoded()
          return wrap(innerPair.fromTree(ov.val))
        },
      }
      return { node, pair }
    }

    /**
     * Build a node + pair for a single AST node, *without* pushing into
     * `nodes`. The caller decides whether to push (for children) or write
     * to slot 0 (for the root).
     */
    const nodeFor = (
      a: SchemaAST.AST,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        switch (a._tag) {
          case "String": {
            // `Char` (from effect-golem/wit-types) annotates Schema.Char
            // with witType: "char" — emit prim-char in that case.
            const witHint = annotationOf<string>(a, witTypeAnnotationKey)
            if (witHint === "char") {
              return {
                node: { tag: "prim-char-type" },
                pair: leafPair("prim-char"),
              }
            }
            return { node: { tag: "prim-string-type" }, pair: leafPair("prim-string") }
          }
          case "Boolean":
            return { node: { tag: "prim-bool-type" }, pair: leafPair("prim-bool") }
          case "Literal": {
            // Single literal value — emitted as the matching primitive type
            // and the TransformPair always echoes the literal back.
            const lit = (a as SchemaAST.Literal).literal
            if (typeof lit === "string") {
              return {
                node: { tag: "prim-string-type" },
                pair: {
                  toTree: () => ({ tag: "prim-string", val: lit }),
                  fromTree: () => lit,
                },
              }
            }
            if (typeof lit === "boolean") {
              return {
                node: { tag: "prim-bool-type" },
                pair: {
                  toTree: () => ({ tag: "prim-bool", val: lit }),
                  fromTree: () => lit,
                },
              }
            }
            if (typeof lit === "number") {
              return {
                node: { tag: "prim-f64-type" },
                pair: {
                  toTree: () => ({ tag: "prim-float64", val: lit }),
                  fromTree: () => lit,
                },
              }
            }
            if (typeof lit === "bigint") {
              return {
                node: { tag: "prim-s64-type" },
                pair: {
                  toTree: () => ({ tag: "prim-s64", val: lit }),
                  fromTree: () => lit,
                },
              }
            }
            return yield* unsupported(`unsupported literal type: ${typeof lit}`)
          }
          case "Number": {
            const kind = numericKindOf(a) ?? "f64"
            return numericNode(kind)
          }
          case "BigInt": {
            const kind = numericKindOf(a) ?? "s64"
            return numericNode(kind)
          }

          case "Objects": {
            if (a.indexSignatures.length > 0) {
              return yield* unsupported("index signatures cannot be represented in WIT")
            }
            return yield* recordNode(a.propertySignatures)
          }

          case "Arrays": {
            if (a.rest.length === 0 && a.elements.length > 0) {
              const elemIdxs: Array<number> = []
              const elemPairs: Array<TransformPair> = []
              for (const el of a.elements) {
                const { idx, pair } = yield* child(el)
                elemIdxs.push(idx)
                elemPairs.push(pair)
              }
              return {
                node: { tag: "tuple-type", val: elemIdxs },
                pair: {
                  toTree: (arr: ReadonlyArray<unknown>) => ({
                    tag: "tuple-value",
                    val: elemPairs.map((p, i) => p.toTree(arr[i])),
                  }),
                  fromTree: (t) => {
                    const tv = t as { val: ReadonlyArray<WitValueTree> }
                    return elemPairs.map((p, i) => p.fromTree(tv.val[i]!))
                  },
                },
              }
            }
            if (a.elements.length === 0 && a.rest.length === 1) {
              const { idx, pair } = yield* child(a.rest[0]!)
              return {
                node: { tag: "list-type", val: idx },
                pair: {
                  toTree: (arr: ReadonlyArray<unknown>) => ({
                    tag: "list-value",
                    val: arr.map((v) => pair.toTree(v)),
                  }),
                  fromTree: (t) => {
                    const lv = t as { val: ReadonlyArray<WitValueTree> }
                    return lv.val.map((c) => pair.fromTree(c))
                  },
                },
              }
            }
            return yield* unsupported("mixed tuple/rest arrays are not supported")
          }

          case "Union":
            return yield* unionNode(a)

          case "Declaration": {
            // Typed-array hints (Uint8ArraySchema, …) take precedence —
            // the Declaration just carries the runtime guard, so we emit
            // a dedicated `list<primN>` shape here rather than treating
            // it as an unknown declared type.
            const tak = typedArrayKindOf(a)
            if (tak !== undefined) {
              const elem = typedArrayElement[tak]
              const elemIdx = push({ tag: elem.typeTag } as WitTypeNode)
              const Ctor = typedArrayCtor[tak]
              return {
                node: { tag: "list-type", val: elemIdx },
                pair: {
                  toTree: (arr) => {
                    const items: Array<WitValueTree> = []
                    for (const v of arr as Iterable<unknown>) {
                      items.push({ tag: elem.valueTag, val: elem.coerce(v) } as WitValueTree)
                    }
                    return { tag: "list-value", val: items }
                  },
                  fromTree: (t) => {
                    const lv = t as { val: ReadonlyArray<WitValueTree> }
                    const raw = lv.val.map((c) => (c as { val: unknown }).val)
                    return new Ctor(raw as Iterable<any>)
                  },
                },
              }
            }
            return yield* declarationNode(a)
          }

          default:
            return yield* unsupported(`unsupported AST node: ${a._tag}`)
        }
      })

    const unionNode = (
      a: SchemaAST.Union,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        // String-literal enum: every member is a string Literal.
        if (
          a.types.length > 0 &&
          a.types.every(
            (m): m is SchemaAST.Literal => SchemaAST.isLiteral(m) && typeof m.literal === "string",
          )
        ) {
          const literals = a.types.map((m) => m.literal as string)
          return {
            node: { tag: "enum-type", val: literals },
            pair: {
              toTree: (s: string) => ({ tag: "enum-value", val: literals.indexOf(s) }),
              fromTree: (t) => literals[(t as { val: number }).val]!,
            },
          }
        }

        // NullOr / UndefinedOr: a union with exactly one Null OR
        // Undefined/Void member and exactly one "real" member maps to
        // WIT `option<inner>`. NullishOr (Null + Undefined + T) is
        // intentionally NOT collapsed — both empty kinds can't
        // round-trip through a single option, so it falls through to
        // the generic variant path.
        const emptyMembers = a.types.filter(isNullOrUndefinedAST)
        const realMembers = a.types.filter((m) => !isNullOrUndefinedAST(m))
        if (emptyMembers.length === 1 && realMembers.length === 1) {
          const empty =
            emptyMembers[0]!._tag === "Null" ? ("null" as const) : ("undefined" as const)
          const { idx, pair } = yield* child(realMembers[0]!)
          return optionWrap(idx, pair, { kind: empty })
        }

        if (a.types.length === 0) {
          return yield* unsupported("empty union")
        }

        // Tagged variant: every member is an Objects with a *required*
        // string-literal _tag. Falls through to generic variant otherwise.
        if (a.types.every(SchemaAST.isObjects)) {
          const tagged = a.types.every((m) => {
            const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag")
            return (
              !!tagPs &&
              !SchemaAST.isOptional(tagPs.type) &&
              SchemaAST.isLiteral(tagPs.type) &&
              typeof tagPs.type.literal === "string"
            )
          })
          if (tagged) {
            return yield* taggedVariantNode(a as SchemaAST.Union<SchemaAST.Objects>)
          }
        }

        // Generic variant: arbitrary union members. Each member becomes a
        // variant case named via {@link withVariantCaseName} or
        // auto-named `caseN`. Encode dispatches on a structural matcher
        // over the *encoded* form, in declaration order.
        return yield* genericVariantNode(a)
      })

    /**
     * Build a tagged variant node + pair where every member is an Objects
     * with a string-literal `_tag` discriminator.
     */
    const taggedVariantNode = (
      a: SchemaAST.Union<SchemaAST.Objects>,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        type Case = {
          readonly tag: string
          readonly payloadIdx: number | undefined
          readonly payloadPair: TransformPair | undefined
        }
        const cases: Array<Case> = []
        const seenTags = new Set<string>()
        for (const m of a.types) {
          const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag")!
          const tag = (tagPs.type as SchemaAST.Literal).literal as string
          if (seenTags.has(tag)) {
            return yield* unsupported(`duplicate variant tag '${tag}' in tagged union`)
          }
          seenTags.add(tag)
          const rest = m.propertySignatures.filter((ps) => ps.name !== "_tag")
          if (rest.length === 0) {
            cases.push({ tag, payloadIdx: undefined, payloadPair: undefined })
          } else {
            const { node, pair } = yield* recordNode(rest)
            const payloadIdx = push(node, nameOf(m) ?? tag)
            cases.push({ tag, payloadIdx, payloadPair: pair })
          }
        }
        const tagToIdx = new Map(cases.map((c, i) => [c.tag, i] as const))
        return {
          node: {
            tag: "variant-type",
            val: cases.map((c) => [c.tag, c.payloadIdx]),
          },
          pair: {
            toTree: (obj: { _tag: string } & Record<string, unknown>) => {
              const i = tagToIdx.get(obj._tag)
              if (i === undefined) throw new Error(`unknown variant tag: ${obj._tag}`)
              const c = cases[i]!
              if (c.payloadPair === undefined) {
                return { tag: "variant-value", val: [i, undefined] }
              }
              const { _tag, ...rest } = obj
              void _tag
              return { tag: "variant-value", val: [i, c.payloadPair.toTree(rest)] }
            },
            fromTree: (t) => {
              const vv = t as {
                val: readonly [number, WitValueTree | undefined]
              }
              const i = vv.val[0]
              const c = cases[i]!
              if (c.payloadPair === undefined || vv.val[1] === undefined) {
                return { _tag: c.tag }
              }
              return { _tag: c.tag, ...c.payloadPair.fromTree(vv.val[1]) }
            },
          },
        }
      })

    /**
     * Build a generic variant for an arbitrary `Schema.Union(...)`. Each
     * member becomes a variant case (auto-named `caseN` or annotated via
     * {@link withVariantCaseName}). Encode picks the first member whose
     * `EncodedShape.matches(encoded)` returns true, in declaration order;
     * decode uses the case index from the WIT value.
     *
     * Compile-time guardrails reject ambiguity that the matcher can't
     * resolve: duplicate primitive shapes (e.g. two `string` members),
     * duplicate identical literals, and duplicate `object-with-tag`s
     * sharing the same discriminator value.
     */
    const genericVariantNode = (
      a: SchemaAST.Union,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        // Conflict detection on encoded shapes.
        const shapes = a.types.map(encodedShapeOf)
        const seen = new Set<string>()
        let plainObjectCount = 0
        let plainArrayCount = 0
        let taggedObjectCount = 0
        const literalTypeKinds = new Set<string>()
        const primitiveKinds = new Set<string>()
        for (const s of shapes) {
          if (s.tag === "object") plainObjectCount++
          if (s.tag === "array") plainArrayCount++
          if (s.tag === "object-with-tag") taggedObjectCount++
          if (s.tag === "literal") literalTypeKinds.add(typeof s.literal)
          if (
            s.tag === "string" ||
            s.tag === "number" ||
            s.tag === "boolean" ||
            s.tag === "bigint"
          ) {
            primitiveKinds.add(s.tag)
          }
          const key =
            s.tag === "literal"
              ? `literal:${typeof s.literal}:${String(s.literal)}`
              : s.tag === "object-with-tag"
                ? `object-with-tag:${s.tagLiteral}`
                : s.tag
          if (s.tag !== "object" && s.tag !== "array" && s.tag !== "unknown" && seen.has(key)) {
            return yield* unsupported(`ambiguous union: two members share encoded shape '${key}'`)
          }
          seen.add(key)
        }
        if (plainObjectCount > 1) {
          return yield* unsupported(
            "ambiguous union: multiple object members without distinct `_tag` discriminators",
          )
        }
        if (plainObjectCount > 0 && taggedObjectCount > 0) {
          return yield* unsupported(
            "ambiguous union: cannot mix plain object members with `_tag`-discriminated object members",
          )
        }
        if (plainArrayCount > 1) {
          return yield* unsupported(
            "ambiguous union: multiple array members cannot be distinguished",
          )
        }
        // A primitive shape (string/number/boolean/bigint) overlaps any
        // literal of the same JS type — declaration order would make the
        // first member "swallow" the literal.
        const primitiveOfLiteralKind: Record<string, string> = {
          string: "string",
          number: "number",
          boolean: "boolean",
          bigint: "bigint",
        }
        for (const litKind of literalTypeKinds) {
          const conflict = primitiveOfLiteralKind[litKind]
          if (conflict !== undefined && primitiveKinds.has(conflict)) {
            return yield* unsupported(
              `ambiguous union: primitive '${conflict}' member overlaps a literal of the same type`,
            )
          }
        }
        // `unknown`-shaped declarations (i.e. any Declaration we recognise
        // for WIT purposes but not in `encodedShapeOf`, e.g. typed arrays)
        // would match every value via the matcher — refuse to mix them
        // into a generic Schema.Union.
        if (shapes.some((s) => s.tag === "unknown") && shapes.length > 1) {
          return yield* unsupported(
            "ambiguous union: contains a member whose encoded shape cannot be classified for variant dispatch",
          )
        }

        type Case = {
          readonly name: string
          readonly idx: number | undefined
          readonly pair: TransformPair | undefined
          readonly matches: (v: unknown) => boolean
          /** Encoded value to reconstruct for unit (payload-less) cases. */
          readonly emptyEncoded: unknown
        }

        const usedNames = new Set<string>()
        const cases: Array<Case> = []
        for (let i = 0; i < a.types.length; i++) {
          const m = a.types[i]!
          const annotated = variantCaseNameOf(m)
          const name = annotated ?? `case${i}`
          if (usedNames.has(name)) {
            return yield* unsupported(`duplicate variant case name '${name}' in Schema.Union`)
          }
          usedNames.add(name)
          const shape = encodedShapeOf(m)
          // Null/Undefined/Void members carry no payload; everything else
          // becomes a payloaded case.
          if (m._tag === "Null" || m._tag === "Undefined" || m._tag === "Void") {
            cases.push({
              name,
              idx: undefined,
              pair: undefined,
              matches: shape.matches,
              emptyEncoded: m._tag === "Null" ? null : undefined,
            })
          } else {
            const { idx, pair } = yield* child(m, annotated ?? nameOf(m))
            cases.push({
              name,
              idx,
              pair,
              matches: shape.matches,
              emptyEncoded: undefined,
            })
          }
        }

        return {
          node: {
            tag: "variant-type",
            val: cases.map((c) => [c.name, c.idx]),
          },
          pair: {
            toTree: (v: unknown) => {
              for (let i = 0; i < cases.length; i++) {
                const c = cases[i]!
                if (c.matches(v)) {
                  if (c.pair === undefined) {
                    return { tag: "variant-value", val: [i, undefined] }
                  }
                  return { tag: "variant-value", val: [i, c.pair.toTree(v)] }
                }
              }
              throw new Error(`Schema.Union: no member matched value of type ${typeof v}`)
            },
            fromTree: (t) => {
              const vv = t as {
                val: readonly [number, WitValueTree | undefined]
              }
              const i = vv.val[0]
              const c = cases[i]!
              if (c.pair === undefined || vv.val[1] === undefined) {
                // Reconstruct the encoded value matching this member: null
                // for Schema.Null, undefined for Schema.Undefined/Void.
                return c.emptyEncoded
              }
              return c.pair.fromTree(vv.val[1])
            },
          },
        }
      })

    /**
     * Recognised `Schema.declareConstructor`-based types are mapped to
     * their corresponding WIT shape. Currently: `Schema.Option`,
     * `Schema.Result`, `Schema.ReadonlyMap`, `Schema.HashMap`.
     */
    const declarationNode = (
      a: SchemaAST.Declaration,
    ): Effect.Effect<{ node: WitTypeNode; pair: TransformPair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const tc = declarationConstructorTag(a)
        switch (tc) {
          case "effect/Option": {
            const inner = a.typeParameters[0]
            if (inner === undefined) {
              return yield* unsupported("Schema.Option without inner type parameter")
            }
            const { idx, pair } = yield* child(inner)
            return optionWrap(idx, pair, { kind: "effect-option" })
          }

          case "effect/Result": {
            const okAst = a.typeParameters[0]
            const errAst = a.typeParameters[1]
            if (okAst === undefined || errAst === undefined) {
              return yield* unsupported("Schema.Result without both type parameters")
            }
            const { idx: okIdx, pair: okPair } = yield* child(okAst)
            const { idx: errIdx, pair: errPair } = yield* child(errAst)
            const node: WitTypeNode = {
              tag: "result-type",
              val: [okIdx, errIdx],
            }
            const pair: TransformPair = {
              toTree: (v) => {
                const r = v as Result.Result<unknown, unknown>
                if (Result.isSuccess(r)) {
                  return {
                    tag: "result-value",
                    val: { tag: "ok", val: okPair.toTree(r.success) },
                  }
                }
                return {
                  tag: "result-value",
                  val: { tag: "err", val: errPair.toTree(r.failure) },
                }
              },
              fromTree: (t) => {
                const rv = (
                  t as {
                    val: { tag: "ok" | "err"; val: WitValueTree | undefined }
                  }
                ).val
                if (rv.tag === "ok") {
                  return Result.succeed(rv.val === undefined ? undefined : okPair.fromTree(rv.val))
                }
                return Result.fail(rv.val === undefined ? undefined : errPair.fromTree(rv.val))
              },
            }
            return { node, pair }
          }

          case "ReadonlyMap":
          case "effect/HashMap": {
            const kAst = a.typeParameters[0]
            const vAst = a.typeParameters[1]
            if (kAst === undefined || vAst === undefined) {
              return yield* unsupported("Schema.ReadonlyMap/HashMap without both type parameters")
            }
            const { idx: kIdx, pair: kPair } = yield* child(kAst)
            const { idx: vIdx, pair: vPair } = yield* child(vAst)
            const tupleIdx = push({ tag: "tuple-type", val: [kIdx, vIdx] })
            const node: WitTypeNode = { tag: "list-type", val: tupleIdx }
            const isHashMap = tc === "effect/HashMap"
            const pair: TransformPair = {
              toTree: (v) => {
                const entries: Iterable<readonly [unknown, unknown]> = isHashMap
                  ? HashMap.toEntries(v as HashMap.HashMap<unknown, unknown>)
                  : (v as ReadonlyMap<unknown, unknown>).entries()
                const items: Array<WitValueTree> = []
                for (const [k, val] of entries) {
                  items.push({
                    tag: "tuple-value",
                    val: [kPair.toTree(k), vPair.toTree(val)],
                  })
                }
                return { tag: "list-value", val: items }
              },
              fromTree: (t) => {
                const lv = t as { val: ReadonlyArray<WitValueTree> }
                const entries: Array<[unknown, unknown]> = lv.val.map((it) => {
                  const tv = it as { val: ReadonlyArray<WitValueTree> }
                  return [kPair.fromTree(tv.val[0]!), vPair.fromTree(tv.val[1]!)]
                })
                return isHashMap ? HashMap.fromIterable(entries) : new Map(entries)
              },
            }
            return { node, pair }
          }

          default:
            return yield* unsupported(`unsupported declaration: ${tc ?? "unknown"}`)
        }
      })

    // Walk on the *encoded* AST so user-defined `decodeTo` chains
    // (Schema.Option, Schema.Result, custom record↔class bridges, …)
    // surface their wire shape rather than their decoded type.
    const encodedAst = SchemaAST.toEncoded(ast)
    const root = yield* nodeFor(encodedAst)
    const rootName = nameOf(encodedAst) ?? nameOf(ast)
    nodes[0] = rootName === undefined ? { type: root.node } : { name: rootName, type: root.node }
    return { witType: { nodes }, pair: root.pair }
  })

/**
 * Build a `WitCodec<S>` for a single Effect Schema. Composes:
 *
 *     userSchema (Type ↔ Encoded)
 *       ↕  per-AST tree transforms (encoded ↔ WitValueTree)
 *       ↕  witGraphCodec (WitValueTree ↔ WitValue)
 *
 * into one `Codec<S["Type"], WitValue>`. Refinements / transformations
 * inside the user's schema run as part of the outer codec, so we get
 * validation and good error messages for free.
 *
 * @since 0.1.0
 * @category codecs
 */
export const toWitCodec = <S extends Schema.Top>(
  schema: S,
): Effect.Effect<WitCodec<S>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const { witType, pair } = yield* walk(schema.ast)

    // Opaque carrier for the user's encoded type — we don't validate it
    // structurally here; the user's own schema does that on the next hop.
    const EncodedCarrier = Schema.declare((_u): _u is S["Encoded"] => true)

    const witToEncoded = witGraphCodec(witType).pipe(
      Schema.decodeTo(EncodedCarrier, {
        decode: SchemaGetter.transform((tree: WitValueTree) => pair.fromTree(tree)),
        encode: SchemaGetter.transform((enc: S["Encoded"]) => pair.toTree(enc)),
      }),
    )

    const codec = witToEncoded.pipe(Schema.decodeTo(schema)) as Schema.Codec<
      S["Type"],
      WitValue,
      S["DecodingServices"],
      S["EncodingServices"]
    >

    return {
      schema,
      witType,
      elementSchema: { tag: "component-model", val: witType },
      codec,
    }
  })
