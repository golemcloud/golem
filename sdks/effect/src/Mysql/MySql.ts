/**
 * Helper namespace for the MySQL adapter — exposes the
 * `MySql.<helper>(...)` tagged-value constructors used inside
 * `effect-golem/mysql` tagged-template parameters to override the
 * conservative default JS-value mapping with explicit MySQL types
 * (json, decimal, date, time, datetime, set, enumeration, bit, blob /
 * binary variants).
 *
 * Sibling of {@link "../Mysql/MySqlClient".MySqlClient} — re-exported
 * by the facade so consumers reach both via
 * `import { MySql, MySqlClient } from "effect-golem/mysql"`.
 *
 * @since 1.5.0
 */
import { type Date as MyDate, type Time, type Timestamp } from "golem:rdbms/mysql@1.5.0"

// ---------------------------------------------------------------------------
// Tagged param machinery (consumed by the codec)
// ---------------------------------------------------------------------------

/**
 * Brand applied to values produced by the {@link MySql} constructors,
 * so the codec can distinguish them from raw JS values.
 *
 * @since 1.5.0
 * @category symbols
 */
export const MySqlParamTag: unique symbol = Symbol.for(
  "effect-golem/MySqlClient/__param",
) as typeof MySqlParamTag

/**
 * Tagged-value envelope produced by every {@link MySql} constructor.
 *
 * @since 1.5.0
 * @category models
 */
export interface MySqlParam<T extends string, V> {
  readonly [MySqlParamTag]: true
  readonly kind: T
  readonly value: V
}

/**
 * Internal factory for {@link MySqlParam} envelopes — used inside this
 * module to build the {@link MySql} constructors. Not part of the
 * public surface.
 *
 * @internal
 * @since 1.5.0
 */
export const mysqlParam = <T extends string, V>(kind: T, value: V): MySqlParam<T, V> => ({
  [MySqlParamTag]: true,
  kind,
  value,
})

/**
 * Probe for the {@link MySqlParam} brand. Used by the codec to detect
 * helper-produced values.
 *
 * @since 1.5.0
 * @category guards
 */
export const isMySqlParam = (v: unknown): v is MySqlParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[MySqlParamTag] === true

// ---------------------------------------------------------------------------
// Public namespace
// ---------------------------------------------------------------------------

/**
 * Explicit parameter wrappers for rich MySQL types. Use inside
 * tagged-template literals to override the conservative default
 * mapping.
 *
 * **Example**
 *
 * ```ts
 * yield* sql`INSERT INTO t (id, data) VALUES (${id}, ${MySql.json({ foo: 1 })})`
 * ```
 *
 * @since 1.5.0
 * @category codecs
 */
export const MySql = {
  /** `json` — encoded as a JSON string. */
  json: (value: unknown) => mysqlParam("json", value),
  /** `enumeration`. */
  enumeration: (value: string) => mysqlParam("enumeration", value),
  /** `set` — raw comma-separated string. */
  set: (value: string) => mysqlParam("set", value),
  /** `bit`. */
  bit: (value: ReadonlyArray<boolean>) => mysqlParam("bit", value),
  /** `decimal` — caller supplies a string to avoid float precision loss. */
  decimal: (value: string) => mysqlParam("decimal", value),
  /** `year` — `YEAR(4)` 1901..2155. */
  year: (value: number) => mysqlParam("year", value),
  // Forced primitive overrides.
  tinyint: (value: number) => mysqlParam("tinyint", value),
  smallint: (value: number) => mysqlParam("smallint", value),
  mediumint: (value: number) => mysqlParam("mediumint", value),
  int: (value: number) => mysqlParam("int", value),
  bigint: (value: number | bigint) => mysqlParam("bigint", value),
  tinyintUnsigned: (value: number) => mysqlParam("tinyint-unsigned", value),
  smallintUnsigned: (value: number) => mysqlParam("smallint-unsigned", value),
  mediumintUnsigned: (value: number) => mysqlParam("mediumint-unsigned", value),
  intUnsigned: (value: number) => mysqlParam("int-unsigned", value),
  bigintUnsigned: (value: number | bigint) => mysqlParam("bigint-unsigned", value),
  float: (value: number) => mysqlParam("float", value),
  double: (value: number) => mysqlParam("double", value),
  fixchar: (value: string) => mysqlParam("fixchar", value),
  varchar: (value: string) => mysqlParam("varchar", value),
  tinytext: (value: string) => mysqlParam("tinytext", value),
  text: (value: string) => mysqlParam("text", value),
  mediumtext: (value: string) => mysqlParam("mediumtext", value),
  longtext: (value: string) => mysqlParam("longtext", value),
  binary: (value: Uint8Array) => mysqlParam("binary", value),
  varbinary: (value: Uint8Array) => mysqlParam("varbinary", value),
  tinyblob: (value: Uint8Array) => mysqlParam("tinyblob", value),
  blob: (value: Uint8Array) => mysqlParam("blob", value),
  mediumblob: (value: Uint8Array) => mysqlParam("mediumblob", value),
  longblob: (value: Uint8Array) => mysqlParam("longblob", value),
  date: (value: MyDate) => mysqlParam("date", value),
  time: (value: Time) => mysqlParam("time", value),
  datetime: (value: Timestamp) => mysqlParam("datetime", value),
  timestamp: (value: Timestamp) => mysqlParam("timestamp", value),
}
