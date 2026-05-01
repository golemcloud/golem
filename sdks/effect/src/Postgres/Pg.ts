/**
 * Helper namespace for the Postgres adapter — exposes the
 * `Pg.<helper>(...)` tagged-value constructors used inside
 * `effect-golem/postgres` tagged-template parameters to override the
 * conservative default JS-value mapping with explicit Postgres types
 * (json, jsonb, uuid, array, range, composite, vector, numeric,
 * interval, inet, cidr, macaddr, bit, …).
 *
 * Sibling of {@link "../Postgres/PgClient".PgClient} — re-exported
 * by the facade so consumers reach both via
 * `import { Pg, PgClient } from "effect-golem/postgres"`.
 *
 * @since 1.5.0
 */
import {
  type Interval,
  type Timestamp,
  type Timestamptz,
  type Uuid,
} from "golem:rdbms/postgres@1.5.0"
import type { IpAddress, MacAddress } from "golem:rdbms/types@1.5.0"

// ---------------------------------------------------------------------------
// Tagged param machinery (consumed by the codec)
// ---------------------------------------------------------------------------

/**
 * Brand applied to values produced by the {@link Pg} constructors,
 * so the codec can distinguish them from raw JS values.
 *
 * @since 1.5.0
 * @category symbols
 */
export const PgParamTag: unique symbol = Symbol.for(
  "effect-golem/PgClient/__param",
) as typeof PgParamTag

/**
 * Tagged-value envelope produced by every {@link Pg} constructor.
 *
 * @since 1.5.0
 * @category models
 */
export interface PgParam<T extends string, V> {
  readonly [PgParamTag]: true
  readonly kind: T
  readonly value: V
}

/**
 * Internal factory for {@link PgParam} envelopes.
 *
 * @internal
 * @since 1.5.0
 */
export const pgParam = <T extends string, V>(kind: T, value: V): PgParam<T, V> => ({
  [PgParamTag]: true,
  kind,
  value,
})

/**
 * Probe for the {@link PgParam} brand. Used by the codec to detect
 * helper-produced values.
 *
 * @since 1.5.0
 * @category guards
 */
export const isPgParam = (v: unknown): v is PgParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[PgParamTag] === true

// ---------------------------------------------------------------------------
// Helper-only types
// ---------------------------------------------------------------------------

/**
 * Pg-only range bound.
 *
 * @since 1.5.0
 * @category models
 */
export type PgBound<T> =
  | { readonly tag: "included"; readonly val: T }
  | { readonly tag: "excluded"; readonly val: T }
  | { readonly tag: "unbounded" }

/**
 * Pg-only range value: closed/open bounds on each side.
 *
 * @since 1.5.0
 * @category models
 */
export interface PgRange<T> {
  readonly start: PgBound<T>
  readonly end: PgBound<T>
}

/**
 * Pg-only sparse-vector value.
 *
 * @since 1.5.0
 * @category models
 */
export interface PgSparseVec {
  readonly dim: number
  readonly indices: ReadonlyArray<number>
  readonly values: ReadonlyArray<number>
}

/**
 * Pg-only IP address (struct mirrored from `golem:rdbms/types@1.5.0`).
 *
 * @since 1.5.0
 * @category models
 */
export type PgIp =
  | {
      readonly tag: "ipv4"
      readonly val: readonly [number, number, number, number]
    }
  | {
      readonly tag: "ipv6"
      readonly val: readonly [number, number, number, number, number, number, number, number]
    }

/**
 * Hint used when constructing `Pg.range(...)` so the encoder knows
 * which range bound variant to emit.
 *
 * @since 1.5.0
 * @category models
 */
export type PgRangeElementHint = "int4" | "int8" | "num" | "ts" | "tstz" | "date"

// ---------------------------------------------------------------------------
// Public namespace
// ---------------------------------------------------------------------------

/**
 * Explicit parameter wrappers for rich Postgres types. Use inside
 * tagged-template literals to override the conservative default
 * mapping.
 *
 * **Example**
 *
 * ```ts
 * yield* sql`INSERT INTO t (id, data) VALUES (${Pg.uuid(id)}, ${Pg.jsonb({ foo: 1 })})`
 * ```
 *
 * @since 1.5.0
 * @category codecs
 */
export const Pg = {
  /** `json` — encoded as a JSON string. */
  json: (value: unknown) => pgParam("json", value),
  /** `jsonb` — encoded as a JSON string. */
  jsonb: (value: unknown) => pgParam("jsonb", value),
  /** `jsonpath` — caller supplies the raw jsonpath expression. */
  jsonpath: (value: string) => pgParam("jsonpath", value),
  /** `xml` — caller supplies a raw XML string. */
  xml: (value: string) => pgParam("xml", value),
  /** `uuid` — accepts a canonical 36-char UUID string or `{highBits, lowBits}`. */
  uuid: (value: string | Uuid) => pgParam("uuid", value),
  /** Generic homogeneous array (each element is encoded recursively). */
  array: (value: ReadonlyArray<unknown>) => pgParam("array", value),
  /** Range type — caller picks the elementary variant via the hint. */
  range: (
    value: PgRange<unknown>,
    hint: PgRangeElementHint,
  ): PgParam<"range", { range: PgRange<unknown>; hint: PgRangeElementHint }> =>
    pgParam("range", { range: value, hint }),
  /** Composite — `Pg.composite("MyType", [Pg.int4(1), Pg.text("a")])`. */
  composite: (name: string, values: ReadonlyArray<unknown>) =>
    pgParam("composite", { name, values }),
  /** Domain — `Pg.domain("MyDomain", baseValue)`. */
  domain: (name: string, value: unknown) => pgParam("domain", { name, value }),
  /** Enumeration — `Pg.enumeration("color", "red")`. */
  enumeration: (name: string, value: string) => pgParam("enumeration", { name, value }),
  /** `vector` — pgvector full-precision vector. */
  vector: (value: ReadonlyArray<number>) => pgParam("vector", value),
  /** `halfvec` — pgvector half-precision vector. */
  halfvec: (value: ReadonlyArray<number>) => pgParam("halfvec", value),
  /** `sparsevec` — pgvector sparse vector. */
  sparsevec: (value: PgSparseVec) => pgParam("sparsevec", value),
  /** `numeric` — caller supplies a string to avoid float precision loss. */
  numeric: (value: string) => pgParam("numeric", value),
  /** `interval`. */
  interval: (value: Interval) => pgParam("interval", value),
  /** `inet`. */
  inet: (value: PgIp | IpAddress) => pgParam("inet", value),
  /** `cidr`. */
  cidr: (value: PgIp | IpAddress) => pgParam("cidr", value),
  /** `macaddr`. */
  macaddr: (value: MacAddress | readonly [number, number, number, number, number, number]) =>
    pgParam("macaddr", value),
  /** `bit`. */
  bit: (value: ReadonlyArray<boolean>) => pgParam("bit", value),
  /** `varbit`. */
  varbit: (value: ReadonlyArray<boolean>) => pgParam("varbit", value),
  // Forced primitive overrides.
  int2: (value: number) => pgParam("int2", value),
  int4: (value: number) => pgParam("int4", value),
  int8: (value: number | bigint) => pgParam("int8", value),
  float4: (value: number) => pgParam("float4", value),
  float8: (value: number) => pgParam("float8", value),
  text: (value: string) => pgParam("text", value),
  varchar: (value: string) => pgParam("varchar", value),
  bpchar: (value: string) => pgParam("bpchar", value),
  character: (value: number) => pgParam("character", value),
  oid: (value: number) => pgParam("oid", value),
  money: (value: bigint) => pgParam("money", value),
  bytea: (value: Uint8Array) => pgParam("bytea", value),
  timestamp: (value: Timestamp) => pgParam("timestamp", value),
  timestamptz: (value: Timestamptz) => pgParam("timestamptz", value),
}
