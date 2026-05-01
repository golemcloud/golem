/**
 * @since 0.1.0
 */
import { Schema } from "effect"

/**
 * Annotation key used by the codec to override the default WIT primitive
 * type for a numeric schema. Values are the `WitTypeNode` primitive tags
 * without the `prim-` / `-type` decorations: `"u8"`, `"u16"`, `"u32"`,
 * `"u64"`, `"s8"`, `"s16"`, `"s32"`, `"s64"`, `"f32"`, `"f64"`.
 *
 * `Schema.Number` defaults to `f64`, `Schema.BigInt` defaults to `s64`.
 * The helpers below pre-apply the right annotation.
 *
 * @since 0.1.0
 * @category utils
 */
export const witTypeAnnotationKey = "effect-golem/witType"

/**
 * Numeric WIT primitive a schema can be pinned to via
 * {@link witTypeAnnotationKey}.
 *
 * @since 0.1.0
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
 * WIT `u8` (`number`, integer 0..255).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint8 = Schema.Number.pipe(Schema.annotate(tag("u8")))

/**
 * WIT `u16` (`number`, integer 0..65535).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint16 = Schema.Number.pipe(Schema.annotate(tag("u16")))

/**
 * WIT `u32` (`number`, integer 0..2^32-1).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint32 = Schema.Number.pipe(Schema.annotate(tag("u32")))

/**
 * WIT `s8` (`number`, integer -128..127).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int8 = Schema.Number.pipe(Schema.annotate(tag("s8")))

/**
 * WIT `s16` (`number`, integer -32768..32767).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int16 = Schema.Number.pipe(Schema.annotate(tag("s16")))

/**
 * WIT `s32` (`number`, integer -2^31..2^31-1).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int32 = Schema.Number.pipe(Schema.annotate(tag("s32")))

/**
 * WIT `f32` (`number`, 32-bit float).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Float32 = Schema.Number.pipe(Schema.annotate(tag("f32")))

/**
 * WIT `f64` (`number`, 64-bit float). Same default as `Schema.Number`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Float64 = Schema.Number.pipe(Schema.annotate(tag("f64")))

/**
 * WIT `s64` (`bigint`). Same default as `Schema.BigInt`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int64 = Schema.BigInt.pipe(Schema.annotate(tag("s64")))

/**
 * WIT `u64` (`bigint`, non-negative).
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint64 = Schema.BigInt.pipe(Schema.annotate(tag("u64")))

/**
 * WIT `char` — a single Unicode scalar value, carried as a JS one-character
 * `string`. Built on top of `Schema.Char` so length-1 validation runs as
 * part of the user schema; the codec sees the `effect-golem/witType: "char"`
 * annotation and emits `prim-char-type` / `prim-char` accordingly.
 *
 * @since 0.1.0
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
 * @since 0.1.0
 * @category utils
 */
export const variantCaseNameAnnotationKey = "effect-golem/variantCaseName"

/**
 * Annotate a schema with the variant case name to use when it appears
 * inside a `Schema.Union(...)` mapped to a WIT `variant`.
 *
 * @since 0.1.0
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
 * @since 0.1.0
 * @category utils
 */
export const witTypedArrayAnnotationKey = "effect-golem/witTypedArray"

/**
 * Typed-array element kind a schema can be pinned to via
 * {@link witTypedArrayAnnotationKey}.
 *
 * @since 0.1.0
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

const typedArraySchema = <T>(kind: WitTypedArrayKind, ctor: new (...args: any[]) => T) =>
  Schema.declare((u): u is T => u instanceof ctor).pipe(
    Schema.annotate({ [witTypedArrayAnnotationKey]: kind }),
  )

/**
 * WIT `list<u8>` carried as a JS `Uint8Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint8ArraySchema = typedArraySchema<Uint8Array>("u8", Uint8Array)

/**
 * WIT `list<s8>` carried as a JS `Int8Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int8ArraySchema = typedArraySchema<Int8Array>("i8", Int8Array)

/**
 * WIT `list<u16>` carried as a JS `Uint16Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint16ArraySchema = typedArraySchema<Uint16Array>("u16", Uint16Array)

/**
 * WIT `list<s16>` carried as a JS `Int16Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int16ArraySchema = typedArraySchema<Int16Array>("i16", Int16Array)

/**
 * WIT `list<u32>` carried as a JS `Uint32Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uint32ArraySchema = typedArraySchema<Uint32Array>("u32", Uint32Array)

/**
 * WIT `list<s32>` carried as a JS `Int32Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Int32ArraySchema = typedArraySchema<Int32Array>("i32", Int32Array)

/**
 * WIT `list<f32>` carried as a JS `Float32Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Float32ArraySchema = typedArraySchema<Float32Array>("f32", Float32Array)

/**
 * WIT `list<f64>` carried as a JS `Float64Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Float64ArraySchema = typedArraySchema<Float64Array>("f64", Float64Array)

/**
 * WIT `list<s64>` carried as a JS `BigInt64Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const BigInt64ArraySchema = typedArraySchema<BigInt64Array>("big-i64", BigInt64Array)

/**
 * WIT `list<u64>` carried as a JS `BigUint64Array`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const BigUint64ArraySchema = typedArraySchema<BigUint64Array>("big-u64", BigUint64Array)
