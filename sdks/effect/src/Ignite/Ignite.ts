/**
 * Helper namespace for the Ignite (Apache Ignite 2.x) adapter —
 * exposes the `Ignite.<helper>(...)` tagged-value constructors used
 * inside `effect-golem/ignite2` tagged-template parameters to override
 * the conservative default JS-value mapping with explicit Ignite types
 * (uuid, decimal, date, timestamp, time, char, byte-array, etc.).
 *
 * Sibling of {@link "../Ignite/IgniteClient".IgniteClient} —
 * re-exported by the facade so consumers reach both via
 * `import { Ignite, IgniteClient } from "effect-golem/ignite2"`.
 *
 * @since 1.5.0
 */

// ---------------------------------------------------------------------------
// Tagged param machinery (consumed by the codec)
// ---------------------------------------------------------------------------

/**
 * Brand applied to values produced by the {@link Ignite} constructors,
 * so the codec can distinguish them from raw JS values.
 *
 * @since 1.5.0
 * @category symbols
 */
export const IgniteParamTag: unique symbol = Symbol.for(
  "effect-golem/IgniteClient/__param",
) as typeof IgniteParamTag

/**
 * Tagged-value envelope produced by every {@link Ignite} constructor.
 *
 * @since 1.5.0
 * @category models
 */
export interface IgniteParam<T extends string, V> {
  readonly [IgniteParamTag]: true
  readonly kind: T
  readonly value: V
}

/**
 * Internal factory for {@link IgniteParam} envelopes.
 *
 * @internal
 * @since 1.5.0
 */
export const igniteParam = <T extends string, V>(kind: T, value: V): IgniteParam<T, V> => ({
  [IgniteParamTag]: true,
  kind,
  value,
})

/**
 * Probe for the {@link IgniteParam} brand. Used by the codec to detect
 * helper-produced values.
 *
 * @since 1.5.0
 * @category guards
 */
export const isIgniteParam = (v: unknown): v is IgniteParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[IgniteParamTag] === true

// ---------------------------------------------------------------------------
// Helper-only types
// ---------------------------------------------------------------------------

/**
 * Ignite uuid — `[hi, lo]` 128-bit identifier.
 *
 * @since 1.5.0
 * @category models
 */
export type IgniteUuid = string | { readonly hi: bigint; readonly lo: bigint } | [bigint, bigint]

// ---------------------------------------------------------------------------
// Public namespace
// ---------------------------------------------------------------------------

/**
 * Explicit parameter wrappers for rich Ignite types.
 *
 * **Example**
 *
 * ```ts
 * yield* sql`INSERT INTO t (id, ts) VALUES (${Ignite.uuid(id)}, ${Ignite.timestamp(BigInt(Date.now()), 0)})`
 * ```
 *
 * @since 1.5.0
 * @category codecs
 */
export const Ignite = {
  /** `db-uuid` — 36-char string OR `{hi,lo}` OR `[hi,lo]` tuple. */
  uuid: (value: IgniteUuid) => igniteParam("uuid", value),
  /** `db-decimal` — caller supplies a string to avoid float precision loss. */
  decimal: (value: string) => igniteParam("decimal", value),
  /** `db-date` — milliseconds since Unix epoch (UTC). */
  date: (value: bigint | number | Date) => igniteParam("date", value),
  /** `db-timestamp` — `(millis, sub-ms-nanos)` tuple (sub-ms in 0..999_999). */
  timestamp: (millis: bigint | number, subMilliNanos: number) =>
    igniteParam("timestamp", { millis, subMilliNanos }),
  /** `db-time` — nanoseconds since midnight. */
  time: (nanos: bigint | number) => igniteParam("time", nanos),
  /** `db-char` — 16-bit Unicode code unit (Java char). */
  char: (codeUnit: number) => igniteParam("char", codeUnit),
  /** `db-byte-array`. */
  byteArray: (value: Uint8Array) => igniteParam("byte-array", value),
  // Forced primitive overrides.
  byte: (value: number) => igniteParam("byte", value),
  short: (value: number) => igniteParam("short", value),
  int: (value: number) => igniteParam("int", value),
  long: (value: number | bigint) => igniteParam("long", value),
  float: (value: number) => igniteParam("float", value),
  double: (value: number) => igniteParam("double", value),
  string: (value: string) => igniteParam("string", value),
  boolean: (value: boolean) => igniteParam("boolean", value),
}
