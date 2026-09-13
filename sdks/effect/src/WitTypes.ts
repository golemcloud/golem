/**
 * @since 1.5.0
 */
import { Effect, Option, Schema, SchemaAST, SchemaIssue } from "effect"
import type {
  BinaryRestrictions,
  Datetime as DatetimeValue,
  DiscriminatorRule,
  MetadataEnvelope,
  PathSpec,
  PermissionCardSpec,
  QuantitySpec,
  QuantityValue,
  TextRestrictions,
  UrlRestrictions,
} from "golem:core/types@2.0.0"
import { AgentStream as AgentStreamValue } from "./AgentStream.js"
import { GuestPermissionCardHandle } from "./internal/schema-model/permissionCardHandle.js"
import { GuestQuotaTokenHandle } from "./internal/schema-model/quotaTokenHandle.js"
import { GuestSecretHandle } from "./internal/schema-model/secretHandle.js"

/**
 * Annotation key used by the codec to override the default WIT primitive
 * type for a numeric schema. Values are the `WitTypeNode` primitive tags
 * without the `prim-` / `-type` decorations: `"u8"`, `"u16"`, `"u32"`,
 * `"u64"`, `"s8"`, `"s16"`, `"s32"`, `"s64"`, `"f32"`, `"f64"`.
 *
 * `Schema.Number` defaults to `f64`, `Schema.BigInt` defaults to `s64`.
 * The helpers below pre-apply the right annotation.
 *
 * @since 1.5.0
 * @category utils
 */
export const witTypeAnnotationKey = "effect-golem/witType"

/**
 * Numeric WIT primitive a schema can be pinned to via
 * {@link witTypeAnnotationKey}.
 *
 * @since 1.5.0
 * @category models
 */
export type WitNumericKind =
  | "u8"
  | "u16"
  | "u32"
  | "u64"
  | "s8"
  | "s16"
  | "s32"
  | "s64"
  | "f32"
  | "f64"

const tag = (kind: WitNumericKind) => ({ [witTypeAnnotationKey]: kind })

/**
 * Annotation key carrying inline numeric min/max/unit restrictions on a numeric
 * pin schema. The codec lowers it to the WIT `numeric-restrictions` payload.
 *
 * @since 1.6.0
 * @category utils
 */
export const witNumericRestrictionsKey = "effect-golem/witNumericRestrictions"

/**
 * User input for {@link restrict}: inclusive `min`/`max` bounds (+ optional
 * display `unit`).
 *
 * @since 1.6.0
 * @category models
 */
export interface NumericRestrictionsInput {
  readonly min?: number | bigint
  readonly max?: number | bigint
  readonly unit?: string
}

/**
 * Restrict a numeric pin to an inclusive `min`/`max` range (+ optional display
 * `unit`), e.g. `Uint8.pipe(restrict({ min: 1, max: 200 }))` or
 * `Schema.Number.pipe(restrict({ max: 100 }))`.
 *
 * The bounds are BOTH (a) enforced at runtime — a value outside the range is
 * rejected during `Schema.decode`/`encode`, i.e. on the invocation boundary —
 * and (b) lowered into the agent-type schema as `numeric-restrictions` for the
 * host. `restrict` accepts ONLY numeric schemas (`number`/`bigint`); applying it
 * to a `String`/`Boolean`/record schema is a compile-time error.
 *
 * @since 1.6.0
 * @category codecs
 */
export const restrict =
  (opts: NumericRestrictionsInput) =>
  <S extends Schema.Schema<number> | Schema.Schema<bigint>>(self: S): S => {
    // Public signature pins `self` to numeric; internal casts thread the
    // number/bigint filter variants through Effect's invariant `.check`.
    let out: any = self
    if (opts.min !== undefined) {
      out = out.check(
        typeof opts.min === "bigint"
          ? Schema.isGreaterThanOrEqualToBigInt(opts.min)
          : Schema.isGreaterThanOrEqualTo(opts.min),
      )
    }
    if (opts.max !== undefined) {
      out = out.check(
        typeof opts.max === "bigint"
          ? Schema.isLessThanOrEqualToBigInt(opts.max)
          : Schema.isLessThanOrEqualTo(opts.max),
      )
    }
    // The codec resolves annotations off the *last* check only. Adding the bound
    // checks above shifts the "last check", so re-carry the pin's `witType` tag
    // (if any) onto this final annotation layer next to the restrictions — else
    // the codec would lose the width and fall back to f64/s64.
    const kind = (
      SchemaAST as unknown as {
        resolveAt: <T>(k: string) => (a: SchemaAST.AST) => T | undefined
      }
    ).resolveAt<WitNumericKind>(witTypeAnnotationKey)((self as Schema.Top).ast)
    const annotations: Record<string, unknown> = { [witNumericRestrictionsKey]: opts }
    if (kind !== undefined) annotations[witTypeAnnotationKey] = kind
    return out.pipe(Schema.annotate(annotations)) as S
  }

/**
 * Integer pin: `Schema.Int` (rejects non-integers/NaN/Infinity) narrowed to the
 * WIT width's inclusive range, then tagged. Enforced on the invocation boundary.
 */
const intPin = (kind: WitNumericKind, min: number, max: number) =>
  Schema.Int.check(Schema.isBetween({ minimum: min, maximum: max })).pipe(
    Schema.annotate(tag(kind)),
  )

/** BigInt pin: `Schema.BigInt` narrowed to the WIT width's inclusive range, then tagged. */
const bigPin = (kind: WitNumericKind, min: bigint, max: bigint) =>
  Schema.BigInt.check(
    Schema.isGreaterThanOrEqualToBigInt(min),
    Schema.isLessThanOrEqualToBigInt(max),
  ).pipe(Schema.annotate(tag(kind)))

/**
 * WIT `u8` (`number`, integer 0..255).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint8 = intPin("u8", 0, 255)

/**
 * WIT `u16` (`number`, integer 0..65535).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint16 = intPin("u16", 0, 65535)

/**
 * WIT `u32` (`number`, integer 0..2^32-1).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint32 = intPin("u32", 0, 4294967295)

/**
 * WIT `s8` (`number`, integer -128..127).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int8 = intPin("s8", -128, 127)

/**
 * WIT `s16` (`number`, integer -32768..32767).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int16 = intPin("s16", -32768, 32767)

/**
 * WIT `s32` (`number`, integer -2^31..2^31-1).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int32 = intPin("s32", -2147483648, 2147483647)

/**
 * WIT `f32` (`number`, 32-bit float).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Float32 = Schema.Number.pipe(Schema.annotate(tag("f32")))

/**
 * WIT `f64` (`number`, 64-bit float). Same default as `Schema.Number`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Float64 = Schema.Number.pipe(Schema.annotate(tag("f64")))

/**
 * WIT `s64` (`bigint`). Same default as `Schema.BigInt`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int64 = bigPin("s64", -9223372036854775808n, 9223372036854775807n)

/**
 * WIT `u64` (`bigint`, non-negative).
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint64 = bigPin("u64", 0n, 18446744073709551615n)

/**
 * WIT `char` — a single Unicode scalar value, carried as a JS one-character
 * `string`. Built on top of `Schema.Char` so length-1 validation runs as
 * part of the user schema; the codec sees the `effect-golem/witType: "char"`
 * annotation and emits `prim-char-type` / `prim-char` accordingly.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Char = Schema.Char.pipe(Schema.annotate({ [witTypeAnnotationKey]: "char" }))

/**
 * Annotation key carrying the *name* of a variant case for a
 * `Schema.Union(...)` member compiled to a generic WIT `variant`. When
 * absent, the codec auto-names cases `case0..caseN`.
 *
 * **Example**
 *
 * ```ts
 * Schema.Union(
 *   Schema.String.pipe(withVariantCaseName("text")),
 *   Schema.Number.pipe(withVariantCaseName("count")),
 * )
 * // → variant { text(string), count(f64) }
 * ```
 *
 * @since 1.5.0
 * @category utils
 */
export const variantCaseNameAnnotationKey = "effect-golem/variantCaseName"

/**
 * Annotate a schema with the variant case name to use when it appears
 * inside a `Schema.Union(...)` mapped to a WIT `variant`.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withVariantCaseName = (name: string) =>
  Schema.annotate({ [variantCaseNameAnnotationKey]: name })

/**
 * Annotation key carrying a typed-array hint: `"u8" | "i8" | "u16" | "i16"
 * | "u32" | "i32" | "f32" | "f64" | "big-i64" | "big-u64"`.
 *
 * Schemas annotated with this key are emitted by the codec as a WIT
 * `list<primN>` (or `list<sN>`/`list<f32>` …) and reconstructed back to
 * the corresponding TypedArray subclass on decode.
 *
 * @since 1.5.0
 * @category utils
 */
export const witTypedArrayAnnotationKey = "effect-golem/witTypedArray"

/**
 * Typed-array element kind a schema can be pinned to via
 * {@link witTypedArrayAnnotationKey}.
 *
 * @since 1.5.0
 * @category models
 */
export type WitTypedArrayKind =
  | "u8"
  | "i8"
  | "u16"
  | "i16"
  | "u32"
  | "i32"
  | "f32"
  | "f64"
  | "big-i64"
  | "big-u64"

/**
 * Annotation key marking a schema as the opaque `quota-token` capability
 * node. Schemas carrying this annotation are compiled by the codec to the
 * schema-model `quota-token` type (`t.quotaToken`) and their values bridge the
 * host `QuotaToken` (an owned `own<quota-token>` resource) to/from a
 * `v.quotaToken(handle)` schema value.
 *
 * Unlike the numeric / typed-array hints there is no payload — the presence of
 * the key alone selects the quota-token shape.
 *
 * @since 1.5.0
 * @category utils
 */
export const witQuotaTokenAnnotationKey = "effect-golem/witQuotaToken"

/**
 * Annotation key marking a schema as a `principal` value carried as ordinary
 * structured data (the WIT `golem:agent/common` `principal` variant —
 * `oidc` / `agent` / `golem-user` / `anonymous`). Schemas carrying this
 * annotation are compiled by the codec to that variant type and their values
 * round-trip a host `Principal` to/from the corresponding `SchemaValue`.
 *
 * Unlike the numeric / typed-array hints there is no payload — the presence of
 * the key alone selects the principal shape. See {@link Principal.PrincipalSchema}.
 *
 * @since 1.6.0
 * @category utils
 */
export const witPrincipalAnnotationKey = "effect-golem/witPrincipal"

/** Annotation key used by Golem-native rich and capability schemas. @since 1.6.0 @category annotations */
export const witSchemaNodeAnnotationKey = "effect-golem/schemaNode"

/** Annotation key for schema-model metadata. @since 1.6.0 @category annotations */
export const witMetadataAnnotationKey = "effect-golem/metadata"

/** Compiler descriptor carried by Golem-native Effect schemas. @since 1.6.0 @category models */
export type WitSchemaNode =
  | { readonly tag: "text"; readonly restrictions: TextRestrictions }
  | { readonly tag: "binary"; readonly restrictions: BinaryRestrictions }
  | { readonly tag: "path"; readonly spec: PathSpec }
  | { readonly tag: "url"; readonly restrictions: UrlRestrictions }
  | { readonly tag: "datetime" }
  | { readonly tag: "duration" }
  | { readonly tag: "quantity"; readonly spec: QuantitySpec }
  | { readonly tag: "secret"; readonly category?: string }
  | { readonly tag: "quota-token"; readonly resourceName?: string }
  | { readonly tag: "permission-card"; readonly spec: PermissionCardSpec }
  | { readonly tag: "stream"; readonly elementSchema?: Schema.Top }
  | { readonly tag: "fixed-list"; readonly length: number }
  | { readonly tag: "map" }
  | { readonly tag: "flags"; readonly names: ReadonlyArray<string> }
  | {
      readonly tag: "discriminated-union"
      readonly branches: ReadonlyArray<DiscriminatedUnionBranch>
    }

/** A named branch in a schema-model discriminated union. @since 1.6.0 @category models */
export interface DiscriminatedUnionBranch<S extends Schema.Top = Schema.Top> {
  readonly tag: string
  readonly schema: S
  readonly discriminator: DiscriminatorRule
}

const native = <A>(guard: (u: unknown) => u is A, node: WitSchemaNode) =>
  Schema.declare(guard).pipe(Schema.annotate({ [witSchemaNodeAnnotationKey]: node }))

/** Attach schema-model metadata to any Effect Schema. @since 1.6.0 @category annotations */
export const metadata =
  (value: Partial<MetadataEnvelope>) =>
  <S extends Schema.Top>(self: S): S =>
    self.pipe(Schema.annotate({ [witMetadataAnnotationKey]: value })) as S

/** Rich text schema. @since 1.6.0 @category schemas */
export const Text = (restrictions: TextRestrictions = {}) =>
  Schema.String.pipe(
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "text", restrictions } }),
  )

/** Rich binary schema. @since 1.6.0 @category schemas */
export const Binary = (restrictions: BinaryRestrictions = {}) =>
  native((u): u is Uint8Array => u instanceof Uint8Array, { tag: "binary", restrictions })

/** Filesystem path schema. @since 1.6.0 @category schemas */
export const Path = (spec: Partial<PathSpec> = {}) =>
  Schema.String.pipe(
    Schema.annotate({
      [witSchemaNodeAnnotationKey]: {
        tag: "path",
        spec: { direction: "in-out", kind: "any", ...spec },
      },
    }),
  )

/** URL schema. @since 1.6.0 @category schemas */
export const Url = (restrictions: UrlRestrictions = {}) =>
  Schema.String.pipe(
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "url", restrictions } }),
  )

const isDatetime = (u: unknown): u is DatetimeValue =>
  typeof u === "object" &&
  u !== null &&
  typeof (u as DatetimeValue).seconds === "bigint" &&
  typeof (u as DatetimeValue).nanoseconds === "number"

/** Canonical datetime schema. @since 1.6.0 @category schemas */
export const Datetime = native(isDatetime, { tag: "datetime" })

/** Nanosecond duration schema. @since 1.6.0 @category schemas */
export const Duration = native((u): u is bigint => typeof u === "bigint", { tag: "duration" })

/** Fixed-point quantity schema. @since 1.6.0 @category schemas */
export const Quantity = (spec: QuantitySpec) =>
  native(
    (u): u is QuantityValue =>
      typeof u === "object" &&
      u !== null &&
      typeof (u as QuantityValue).mantissa === "bigint" &&
      Number.isInteger((u as QuantityValue).scale) &&
      typeof (u as QuantityValue).unit === "string",
    { tag: "quantity", spec },
  )

/** Opaque secret capability schema. @since 1.6.0 @category schemas */
export const Secret = <S extends Schema.Top>(
  inner: S,
  options: { readonly category?: string } = {},
) =>
  Schema.declareConstructor<GuestSecretHandle>()(
    [inner],
    () => (u, ast) =>
      u instanceof GuestSecretHandle
        ? Effect.succeed(u)
        : Effect.fail(new SchemaIssue.InvalidType(ast, Option.some(u))),
  ).pipe(Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "secret", ...options } }))

/** Opaque permission-card capability schema. @since 1.6.0 @category schemas */
export const PermissionCard = (spec: PermissionCardSpec) =>
  native((u): u is GuestPermissionCardHandle => u instanceof GuestPermissionCardHandle, {
    tag: "permission-card",
    spec,
  })

/** Agent stream capability schema. @since 1.6.0 @category schemas */
export const AgentStream = <S extends Schema.Top>(element: S) =>
  Schema.declareConstructor<AgentStreamValue<S["Type"]>>()(
    [element],
    () => (u, ast) =>
      u instanceof AgentStreamValue
        ? Effect.succeed(u as AgentStreamValue<S["Type"]>)
        : Effect.fail(new SchemaIssue.InvalidType(ast, Option.some(u))),
  ).pipe(
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "stream", elementSchema: element } }),
  )

/** Fixed-length list schema. @since 1.6.0 @category schemas */
export const FixedList = <S extends Schema.Top>(element: S, length: number) =>
  Schema.Array(element).pipe(
    Schema.check(
      Schema.makeFilter((a) => a.length === length || `Expected exactly ${length} items`),
    ),
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "fixed-list", length } }),
  )

/** Opaque quota-token capability schema. @since 1.6.0 @category schemas */
export const QuotaToken = (options: { readonly resourceName?: string } = {}) =>
  native((u): u is GuestQuotaTokenHandle => u instanceof GuestQuotaTokenHandle, {
    tag: "quota-token",
    ...options,
  })

/** Native schema-model map backed by a JavaScript Map. @since 1.6.0 @category schemas */
export const Map = <K extends Schema.Top, V extends Schema.Top>(key: K, value: V) =>
  Schema.ReadonlyMap(key, value).pipe(
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "map" } }),
  )

/** WIT flags represented by a boolean tuple in declaration order. @since 1.6.0 @category schemas */
export const Flags = <const Names extends readonly [string, ...string[]]>(names: Names) =>
  Schema.Tuple(
    names.map(() => Schema.Boolean) as [typeof Schema.Boolean, ...(typeof Schema.Boolean)[]],
  ).pipe(
    Schema.annotate({ [witSchemaNodeAnnotationKey]: { tag: "flags", names: [...names] } }),
  ) as Schema.Codec<
    { readonly [K in keyof Names]: boolean },
    { readonly [K in keyof Names]: boolean }
  >

/**
 * A closed union selected by explicit schema-model discriminator rules.
 * Branch tags are carried on the wire; branch values remain unchanged in TypeScript.
 * @since 1.6.0
 * @category schemas
 */
export const DiscriminatedUnion = <
  const Branches extends readonly [DiscriminatedUnionBranch, ...DiscriminatedUnionBranch[]],
>(
  branches: Branches,
) =>
  Schema.Union(branches.map((branch) => branch.schema) as [Schema.Top, ...Schema.Top[]]).pipe(
    Schema.annotate({
      [witSchemaNodeAnnotationKey]: { tag: "discriminated-union", branches: [...branches] },
    }),
  ) as unknown as Schema.Codec<
    Branches[number]["schema"]["Type"],
    Branches[number]["schema"]["Encoded"],
    Branches[number]["schema"]["DecodingServices"],
    Branches[number]["schema"]["EncodingServices"]
  >

const typedArraySchema = <T>(kind: WitTypedArrayKind, ctor: new (...args: any[]) => T) =>
  Schema.declare((u): u is T => u instanceof ctor).pipe(
    Schema.annotate({ [witTypedArrayAnnotationKey]: kind }),
  )

/**
 * WIT `list<u8>` carried as a JS `Uint8Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint8ArraySchema = typedArraySchema<Uint8Array>("u8", Uint8Array)

/**
 * WIT `list<s8>` carried as a JS `Int8Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int8ArraySchema = typedArraySchema<Int8Array>("i8", Int8Array)

/**
 * WIT `list<u16>` carried as a JS `Uint16Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint16ArraySchema = typedArraySchema<Uint16Array>("u16", Uint16Array)

/**
 * WIT `list<s16>` carried as a JS `Int16Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int16ArraySchema = typedArraySchema<Int16Array>("i16", Int16Array)

/**
 * WIT `list<u32>` carried as a JS `Uint32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Uint32ArraySchema = typedArraySchema<Uint32Array>("u32", Uint32Array)

/**
 * WIT `list<s32>` carried as a JS `Int32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Int32ArraySchema = typedArraySchema<Int32Array>("i32", Int32Array)

/**
 * WIT `list<f32>` carried as a JS `Float32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Float32ArraySchema = typedArraySchema<Float32Array>("f32", Float32Array)

/**
 * WIT `list<f64>` carried as a JS `Float64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const Float64ArraySchema = typedArraySchema<Float64Array>("f64", Float64Array)

/**
 * WIT `list<s64>` carried as a JS `BigInt64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const BigInt64ArraySchema = typedArraySchema<BigInt64Array>("big-i64", BigInt64Array)

/**
 * WIT `list<u64>` carried as a JS `BigUint64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
export const BigUint64ArraySchema = typedArraySchema<BigUint64Array>("big-u64", BigUint64Array)
