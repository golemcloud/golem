/**
 * Postgres adapter for `effect-golem` agents — exposes the official
 * `effect/unstable/sql/SqlClient` interface on top of Golem's
 * `golem:rdbms/postgres@1.5.0` host bindings.
 *
 * The adapter is consumed via the `effect-golem/postgres` sub-import.
 * Inside the Golem `wasm-rquickjs` runtime the bindings come from the
 * embedded base WASM; for Node tests they are aliased to in-memory
 * fakes via `vitest.config.ts`.
 *
 * Why not `@effect/sql-pg`? That package depends on the native `pg`
 * driver which cannot run inside `wasm-rquickjs`. This module mirrors
 * the structure of `@effect/sql-pg`'s adapter (so users can use
 * `SqlSchema`, `SqlResolver`, `Migrator` against it) but delegates to
 * the host's `DbConnection` / `DbTransaction` resources.
 *
 * Highlights:
 * - Compiler dialect: `"pg"` (postgres-style `$1` placeholders, `"`-quoted identifiers).
 * - Transaction handling: built on top of `Client.make`, with the
 *   `withTransaction` member replaced by a custom
 *   `Client.makeWithTransaction(...)` wired directly to
 *   `DbConnection.beginTransaction()` / `tx.commit()` / `tx.rollback()`.
 *   Nested calls use `SAVEPOINT effect_sql_<id>` issued via the same
 *   `DbTransaction` resource.
 * - Two locks: a client-wide semaphore guarding the underlying
 *   `DbConnection` (so non-tx queries serialise and concurrent
 *   `withTransaction` callers wait), and a per-`DbTransaction`
 *   semaphore so concurrent fibers inside a tx serialise on the
 *   resource.
 * - Explicit param encoding: rich Postgres types (json, jsonb, uuid,
 *   array, range, composite, vector, numeric, interval, inet, cidr,
 *   macaddr, bit) require an explicit `Pg.<helper>(...)` call. Plain
 *   JS values are mapped conservatively (string→text, integer→int4 or
 *   int8, finite float→float8, bigint→int8, boolean, Date→timestamptz,
 *   Uint8Array→bytea, null/undefined→null). NaN / ±Infinity are
 *   rejected.
 * - Conservative row decoding: numeric scalars unwrap to JS values,
 *   uuid `{highBits,lowBits}` decode to canonical 36-char strings,
 *   bytes stay as `Uint8Array`, JSON stays as a string. Temporal
 *   values stay as raw structs unless `decodeTemporal: "date"` is
 *   set, in which case `timestamp` / `timestamptz` decode to JS
 *   `Date` (UTC).
 * - SELECT / `RETURNING` queries are routed to `conn.query`; DDL and
 *   simple writes go to `conn.execute` (which returns the affected-row
 *   `bigint`).
 * - Streaming: `executeStream` opens a `DbResultStream`, holds the
 *   per-connection lock for its lifetime, and uses `Stream.unwrap`
 *   over a scoped effect so the permit is released on stream
 *   completion AND on early interruption.
 *
 * Resource lifecycle: the WIT host bindings only expose synchronous
 * `Db*` resource constructors / methods — there is no public `close`
 * on `DbConnection`. The adapter relies on the host's GC to free
 * resources once the JS handle becomes unreachable. We deliberately
 * do **not** ship a `fromConnection` constructor for v1 because the
 * caller would have no way to clean up.
 *
 * @since 0.1.0
 */
import {
  Context,
  type Duration,
  Effect,
  Exit,
  Layer,
  Option,
  Scope,
  Semaphore,
  Stream,
} from "effect"
import * as Reactivity from "effect/unstable/reactivity/Reactivity"
import * as Client from "effect/unstable/sql/SqlClient"
import type { Acquirer, Connection } from "effect/unstable/sql/SqlConnection"
import { SqlError } from "effect/unstable/sql/SqlError"
import * as Statement from "effect/unstable/sql/Statement"
import {
  type DbColumn,
  type DbConnection,
  type DbResultStream,
  type DbRow,
  type DbTransaction,
  type DbValue,
  type Interval,
  LazyDbValue,
  type Range,
  type Timestamp,
  type Timestamptz,
  type Uuid,
  type ValuesRange,
} from "golem:rdbms/postgres@1.5.0"
import type { IpAddress, MacAddress } from "golem:rdbms/types@1.5.0"
import { PostgresHostClient } from "./host/PostgresHostClient.js"
import {
  ParamEncodingError,
  READ_PREFIX_RE,
  RETURNING_RE,
  sqlErrorFor,
  toBigIntChecked,
} from "./RdbmsShared.js"

const ATTR_DB_SYSTEM_NAME = "db.system.name"

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * Unique symbol stamped on every {@link PgClient} instance so consumers
 * can reliably distinguish a postgres client. Keyed via `Symbol.for`
 * so multiple module copies (e.g. one bundled inside `effect-golem`
 * and one in the standalone `effect-golem/postgres` sub-import) still
 * agree on the same key.
 *
 * @since 0.1.0
 * @category symbols
 */
export const PgClientTypeId: unique symbol = Symbol.for("effect-golem/PgClient") as PgClientTypeId

/**
 * @since 0.1.0
 * @category symbols
 */
export type PgClientTypeId = typeof PgClientTypeId

const PgConnectionTxSymbol: unique symbol = Symbol.for(
  "effect-golem/PgClient/__txContext",
) as typeof PgConnectionTxSymbol

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * How temporal values (timestamp/timestamptz/date/time) are decoded from rows.
 *
 * @since 0.1.0
 * @category models
 */
export type TemporalDecodeMode = "raw" | "date"

/**
 * Configuration accepted by {@link PgClient.make} / {@link PgClient.layer}.
 *
 * @since 0.1.0
 * @category models
 */
export interface PgClientConfig {
  /**
   * Postgres connection address — e.g.
   * `postgres://user:pass@host:5432/dbname`. Passed verbatim to
   * `DbConnection.open(...)`.
   */
  readonly connectionAddress: string
  readonly transformResultNames?: ((str: string) => string) | undefined
  readonly transformQueryNames?: ((str: string) => string) | undefined
  /**
   * How to decode `timestamp`, `timestamptz`, `date`, `time`, `timetz`
   * values from query rows.
   *
   * - `"raw"` (default): keep the raw struct from the host as-is. No
   *   precision loss, but consumers must pattern-match on the struct
   *   shape.
   * - `"date"`: decode `timestamp` / `timestamptz` to JS `Date`.
   *   `date` becomes a `Date` at UTC midnight; `time` and `timetz`
   *   stay raw because JS `Date` cannot represent them faithfully.
   */
  readonly decodeTemporal?: TemporalDecodeMode | undefined
  /** Span attributes (in addition to the auto-injected `db.system.name`). */
  readonly spanAttributes?: Record<string, unknown> | undefined
  /** Reserved for future use. */
  readonly prepareCacheSize?: number | undefined
  readonly prepareCacheTTL?: Duration.Input | undefined
}

/**
 * The public PgClient — extends the official
 * `effect/unstable/sql/SqlClient` so users can write
 * `yield* sql\`SELECT ...\`` queries, compose with `SqlSchema` /
 * `SqlResolver` / `Migrator`, and resolve the canonical
 * `Client.SqlClient` tag.
 *
 * @since 0.1.0
 * @category models
 */
export interface PgClient extends Client.SqlClient {
  readonly [PgClientTypeId]: PgClientTypeId
  readonly config: PgClientConfig
  /** Multi-row update isn't surfaced — matches @effect/sql-pg's surface. */
  readonly updateValues: never
}

// ---------------------------------------------------------------------------
// Effect Context tag
// ---------------------------------------------------------------------------

/**
 * Context tag for resolving a {@link PgClient} from the environment.
 * Both this tag and the upstream `Client.SqlClient` tag are populated
 * by {@link PgClient.layer}.
 *
 * @since 0.1.0
 * @category host services
 */
export class PgClientService extends Context.Service<PgClientService, PgClient>()(
  "effect-golem/PgClient",
) {}

// ---------------------------------------------------------------------------
// Pg helpers — explicit sentinel wrappers used in tagged-template params
// ---------------------------------------------------------------------------

const PgParamTag: unique symbol = Symbol.for("effect-golem/PgClient/__param") as typeof PgParamTag

interface PgParam<T extends string, V> {
  readonly [PgParamTag]: true
  readonly kind: T
  readonly value: V
}

const pgParam = <T extends string, V>(kind: T, value: V): PgParam<T, V> => ({
  [PgParamTag]: true,
  kind,
  value,
})

const isPgParam = (v: unknown): v is PgParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[PgParamTag] === true

/**
 * Pg-only range bound.
 *
 * @since 0.1.0
 * @category models
 */
export type PgBound<T> =
  | { readonly tag: "included"; readonly val: T }
  | { readonly tag: "excluded"; readonly val: T }
  | { readonly tag: "unbounded" }

/**
 * Pg-only range value: closed/open bounds on each side.
 *
 * @since 0.1.0
 * @category models
 */
export interface PgRange<T> {
  readonly start: PgBound<T>
  readonly end: PgBound<T>
}

/**
 * Pg-only sparse-vector value.
 *
 * @since 0.1.0
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
 * @since 0.1.0
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
 * @since 0.1.0
 * @category models
 */
export type PgRangeElementHint = "int4" | "int8" | "num" | "ts" | "tstz" | "date"

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
 * @since 0.1.0
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

// ---------------------------------------------------------------------------
// Param encoding — JS / Pg helpers → DbValue
// ---------------------------------------------------------------------------

const NULL_DB_VALUE: DbValue = { tag: "null" }

const I32_MIN = -2_147_483_648
const I32_MAX = 2_147_483_647
const I64_MIN = BigInt("-9223372036854775808")
const I64_MAX = BigInt("9223372036854775807")

const isSafeInt32 = (n: number): boolean => Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX

const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of int8 range`)
  }
}

const encodeUuid = (input: string | Uuid): Uuid => {
  if (typeof input !== "string") return input
  const normalized = input.replace(/-/g, "").toLowerCase()
  if (!/^[0-9a-f]{32}$/.test(normalized)) {
    throw new ParamEncodingError(`invalid uuid: ${input}`)
  }
  const hi = BigInt("0x" + normalized.slice(0, 16))
  const lo = BigInt("0x" + normalized.slice(16, 32))
  return { highBits: hi, lowBits: lo }
}

const encodeMacAddr = (
  input: MacAddress | readonly [number, number, number, number, number, number],
): MacAddress => {
  if (Array.isArray(input)) {
    return {
      octets: [input[0], input[1], input[2], input[3], input[4], input[5]] as MacAddress["octets"],
    }
  }
  return input as MacAddress
}

const dateToTimestamptz = (d: Date): Timestamptz => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return {
    timestamp: {
      date: { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() },
      time: {
        hour: d.getUTCHours(),
        minute: d.getUTCMinutes(),
        second: d.getUTCSeconds(),
        nanosecond: d.getUTCMilliseconds() * 1_000_000,
      },
    },
    offset: 0,
  }
}

const encodeIp = (input: PgIp | IpAddress): IpAddress => input as IpAddress

interface Int4Bound {
  tag: "included" | "excluded" | "unbounded"
  val?: number
}
interface Int8Bound {
  tag: "included" | "excluded" | "unbounded"
  val?: bigint
}
interface NumBound {
  tag: "included" | "excluded" | "unbounded"
  val?: string
}
interface TsBound {
  tag: "included" | "excluded" | "unbounded"
  val?: Timestamp
}
interface TstzBound {
  tag: "included" | "excluded" | "unbounded"
  val?: Timestamptz
}
interface DateBound {
  tag: "included" | "excluded" | "unbounded"
  val?: { year: number; month: number; day: number }
}

const encodeRangeBoundTyped = <T>(
  bound: PgBound<unknown>,
  cast: (v: unknown) => T,
): { tag: "included" | "excluded"; val: T } | { tag: "unbounded" } =>
  bound.tag === "unbounded"
    ? { tag: "unbounded" as const }
    : { tag: bound.tag, val: cast(bound.val) }

/**
 * Convert a single template-literal parameter to a `DbValue`. Throws
 * a `ParamEncodingError` for unsupported shapes; the caller wraps it
 * into a `SqlError`.
 */
const encodeDbValue = (value: unknown): DbValue => {
  if (value === null || value === undefined) return NULL_DB_VALUE
  if (isPgParam(value)) {
    return encodePgParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "text", val: value }
    case "boolean":
      return { tag: "boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "int8", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to postgres; use Pg.numeric(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "int4", val: value }
      }
      if (Number.isInteger(value)) {
        return { tag: "int8", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "float8", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "bytea", val: value }
  }
  if (value instanceof Date) {
    return { tag: "timestamptz", val: dateToTimestamptz(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a Pg.<helper>(...) wrapper`,
  )
}

const encodePgParam = (param: PgParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "json":
      return { tag: "json", val: JSON.stringify(param.value) }
    case "jsonb":
      return { tag: "jsonb", val: JSON.stringify(param.value) }
    case "jsonpath":
      return { tag: "jsonpath", val: param.value as string }
    case "xml":
      return { tag: "xml", val: param.value as string }
    case "uuid":
      return { tag: "uuid", val: encodeUuid(param.value as string | Uuid) }
    case "array": {
      const arr = param.value as ReadonlyArray<unknown>
      return { tag: "array", val: arr.map((v) => new LazyDbValue(encodeDbValue(v))) }
    }
    case "range": {
      const { range, hint } = param.value as {
        range: PgRange<unknown>
        hint: PgRangeElementHint
      }
      switch (hint) {
        case "int4": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as number) as Int4Bound
          const end = encodeRangeBoundTyped(range.end, (v) => v as number) as Int4Bound
          return { tag: "int4range", val: { start: start as never, end: end as never } }
        }
        case "int8": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as bigint) as Int8Bound
          const end = encodeRangeBoundTyped(range.end, (v) => v as bigint) as Int8Bound
          return { tag: "int8range", val: { start: start as never, end: end as never } }
        }
        case "num": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as string) as NumBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as string) as NumBound
          return { tag: "numrange", val: { start: start as never, end: end as never } }
        }
        case "ts": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as Timestamp) as TsBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as Timestamp) as TsBound
          return { tag: "tsrange", val: { start: start as never, end: end as never } }
        }
        case "tstz": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as Timestamptz) as TstzBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as Timestamptz) as TstzBound
          return { tag: "tstzrange", val: { start: start as never, end: end as never } }
        }
        case "date": {
          const start = encodeRangeBoundTyped(
            range.start,
            (v) => v as { year: number; month: number; day: number },
          ) as DateBound
          const end = encodeRangeBoundTyped(
            range.end,
            (v) => v as { year: number; month: number; day: number },
          ) as DateBound
          return { tag: "daterange", val: { start: start as never, end: end as never } }
        }
      }
      break
    }
    case "composite": {
      const { name, values } = param.value as {
        name: string
        values: ReadonlyArray<unknown>
      }
      return {
        tag: "composite",
        val: {
          name,
          values: values.map((v) => new LazyDbValue(encodeDbValue(v))),
        },
      }
    }
    case "domain": {
      const { name, value } = param.value as { name: string; value: unknown }
      return {
        tag: "domain",
        val: { name, value: new LazyDbValue(encodeDbValue(value)) },
      }
    }
    case "enumeration": {
      const { name, value } = param.value as { name: string; value: string }
      return { tag: "enumeration", val: { name, value } }
    }
    case "vector":
      return { tag: "vector", val: (param.value as ReadonlyArray<number>).slice() }
    case "halfvec":
      return { tag: "halfvec", val: (param.value as ReadonlyArray<number>).slice() }
    case "sparsevec": {
      const sv = param.value as PgSparseVec
      return {
        tag: "sparsevec",
        val: {
          dim: sv.dim,
          indices: sv.indices.slice(),
          values: sv.values.slice(),
        },
      }
    }
    case "numeric":
      return { tag: "numeric", val: param.value as string }
    case "interval":
      return { tag: "interval", val: param.value as Interval }
    case "inet":
      return { tag: "inet", val: encodeIp(param.value as PgIp | IpAddress) }
    case "cidr":
      return { tag: "cidr", val: encodeIp(param.value as PgIp | IpAddress) }
    case "macaddr":
      return {
        tag: "macaddr",
        val: encodeMacAddr(
          param.value as MacAddress | readonly [number, number, number, number, number, number],
        ),
      }
    case "bit":
      return { tag: "bit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "varbit":
      return { tag: "varbit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "int2":
      return { tag: "int2", val: param.value as number }
    case "int4":
      return { tag: "int4", val: param.value as number }
    case "int8": {
      const b = toBigIntChecked(param.value as number | bigint, "Pg.int8")
      checkInt64(b)
      return { tag: "int8", val: b }
    }
    case "float4":
      return { tag: "float4", val: param.value as number }
    case "float8":
      return { tag: "float8", val: param.value as number }
    case "text":
      return { tag: "text", val: param.value as string }
    case "varchar":
      return { tag: "varchar", val: param.value as string }
    case "bpchar":
      return { tag: "bpchar", val: param.value as string }
    case "character":
      return { tag: "character", val: param.value as number }
    case "oid":
      return { tag: "oid", val: param.value as number }
    case "money":
      return { tag: "money", val: param.value as bigint }
    case "bytea":
      return { tag: "bytea", val: param.value as Uint8Array }
    case "timestamp":
      return { tag: "timestamp", val: param.value as Timestamp }
    case "timestamptz":
      return { tag: "timestamptz", val: param.value as Timestamptz }
    default:
      throw new ParamEncodingError(`unknown Pg helper: ${param.kind}`)
  }
  throw new ParamEncodingError(`unhandled Pg helper kind: ${param.kind}`)
}

const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

const uuidToString = (uuid: Uuid): string => {
  const hi = uuid.highBits.toString(16).padStart(16, "0")
  const lo = uuid.lowBits.toString(16).padStart(16, "0")
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`
}

const timestampToDate = (ts: Timestamp): Date => {
  const ms = Math.floor(ts.time.nanosecond / 1_000_000)
  return new Date(
    Date.UTC(
      ts.date.year,
      ts.date.month - 1,
      ts.date.day,
      ts.time.hour,
      ts.time.minute,
      ts.time.second,
      ms,
    ),
  )
}

const timestamptzToDate = (tstz: Timestamptz): Date => {
  // Offset is seconds east of UTC. To get the UTC instant we subtract.
  const baseMs = timestampToDate(tstz.timestamp).getTime()
  return new Date(baseMs - tstz.offset * 1000)
}

const dateOnlyToDate = (d: { year: number; month: number; day: number }): Date =>
  new Date(Date.UTC(d.year, d.month - 1, d.day))

const decodeBound = (
  bound: ValuesRange["start"] | ValuesRange["end"],
  decodeTemporal: TemporalDecodeMode,
): unknown => {
  if (bound.tag === "unbounded") return { tag: "unbounded" as const }
  const inner = bound.val.get()
  return { tag: bound.tag, val: decodeDbValue(inner, decodeTemporal) }
}

const decodeRange = (range: Range, decodeTemporal: TemporalDecodeMode) => ({
  name: range.name,
  value: {
    start: decodeBound(range.value.start, decodeTemporal),
    end: decodeBound(range.value.end, decodeTemporal),
  },
})

const decodeDbValue = (value: DbValue, decodeTemporal: TemporalDecodeMode): unknown => {
  switch (value.tag) {
    case "null":
      return null
    case "character":
    case "int2":
    case "int4":
    case "float4":
    case "float8":
    case "oid":
      return value.val
    case "int8":
    case "money":
      return value.val
    case "numeric":
      return value.val
    case "boolean":
      return value.val
    case "text":
    case "varchar":
    case "bpchar":
    case "json":
    case "jsonb":
    case "jsonpath":
    case "xml":
      return value.val
    case "bytea":
      return value.val
    case "uuid":
      return uuidToString(value.val)
    case "timestamp":
      return decodeTemporal === "date" ? timestampToDate(value.val) : value.val
    case "timestamptz":
      return decodeTemporal === "date" ? timestamptzToDate(value.val) : value.val
    case "date":
      return decodeTemporal === "date" ? dateOnlyToDate(value.val) : value.val
    case "time":
    case "timetz":
    case "interval":
    case "inet":
    case "cidr":
    case "macaddr":
      return value.val
    case "bit":
    case "varbit":
      return value.val
    case "int4range":
    case "int8range":
    case "numrange":
    case "tsrange":
    case "tstzrange":
    case "daterange":
      return value.val
    case "enumeration":
      return value.val
    case "composite":
      return {
        name: value.val.name,
        values: value.val.values.map((lv) => decodeDbValue(lv.get(), decodeTemporal)),
      }
    case "domain":
      return {
        name: value.val.name,
        value: decodeDbValue(value.val.value.get(), decodeTemporal),
      }
    case "array":
      return value.val.map((lv) => decodeDbValue(lv.get(), decodeTemporal))
    case "range":
      return decodeRange(value.val, decodeTemporal)
    case "vector":
    case "halfvec":
      return value.val
    case "sparsevec":
      return value.val
  }
}

const decodeRows = (
  rows: ReadonlyArray<DbRow>,
  columns: ReadonlyArray<DbColumn>,
  decodeTemporal: TemporalDecodeMode,
): Array<Record<string, unknown>> => {
  const colNames = columns.map((c) => c.name)
  return rows.map((row) => {
    const obj: Record<string, unknown> = {}
    for (let i = 0; i < colNames.length; i++) {
      obj[colNames[i]!] = decodeDbValue(row.values[i]!, decodeTemporal)
    }
    return obj
  })
}

const decodeRowsValues = (
  rows: ReadonlyArray<DbRow>,
  decodeTemporal: TemporalDecodeMode,
): Array<Array<unknown>> =>
  rows.map((row) => row.values.map((v) => decodeDbValue(v, decodeTemporal)))

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

const sqlError = sqlErrorFor()

// ---------------------------------------------------------------------------
// Connection wrappers
// ---------------------------------------------------------------------------

interface BaseTarget {
  readonly _tag: "base"
  readonly db: DbConnection
  readonly clientLock: Semaphore.Semaphore
}

interface TxTarget {
  readonly _tag: "tx"
  readonly tx: DbTransaction
  readonly txLock: Semaphore.Semaphore
}

type Target = BaseTarget | TxTarget

const targetLock = (t: Target): Semaphore.Semaphore => (t._tag === "base" ? t.clientLock : t.txLock)

const isReader = (sql: string): boolean => READ_PREFIX_RE.test(sql) || RETURNING_RE.test(sql)

const targetQuery = (target: Target, sql: string, params: Array<DbValue>) =>
  target._tag === "base" ? target.db.query(sql, params) : target.tx.query(sql, params)

const targetExecute = (target: Target, sql: string, params: Array<DbValue>): bigint =>
  target._tag === "base" ? target.db.execute(sql, params) : target.tx.execute(sql, params)

const targetQueryStream = (target: Target, sql: string, params: Array<DbValue>): DbResultStream =>
  target._tag === "base" ? target.db.queryStream(sql, params) : target.tx.queryStream(sql, params)

const buildConnection = (target: Target, decodeTemporal: TemporalDecodeMode): Connection => {
  const lock = targetLock(target)
  const withLock = <A, E, R>(eff: Effect.Effect<A, E, R>): Effect.Effect<A, E, R> =>
    lock.withPermits(1)(eff)

  const runQuery = (
    sql: string,
    params: ReadonlyArray<unknown>,
    raw: boolean,
  ): Effect.Effect<unknown, SqlError> =>
    withLock(
      Effect.try({
        try: () => {
          const encoded = encodeAllParams(params)
          if (isReader(sql)) {
            const result = targetQuery(target, sql, encoded)
            const decoded = decodeRows(result.rows, result.columns, decodeTemporal)
            if (raw) {
              return {
                columns: result.columns.map((c) => ({
                  ordinal: c.ordinal,
                  name: c.name,
                  dbType: c.dbType,
                  dbTypeName: c.dbTypeName,
                })),
                rows: decoded,
              }
            }
            return decoded
          }
          const affected = targetExecute(target, sql, encoded)
          if (raw) return affected
          return []
        },
        catch: (cause) => sqlError(cause, "Failed to execute statement", "execute"),
      }),
    )

  const runValues = (
    sql: string,
    params: ReadonlyArray<unknown>,
  ): Effect.Effect<ReadonlyArray<ReadonlyArray<unknown>>, SqlError> =>
    withLock(
      Effect.try({
        try: () => {
          const encoded = encodeAllParams(params)
          if (isReader(sql)) {
            const result = targetQuery(target, sql, encoded)
            return decodeRowsValues(result.rows, decodeTemporal)
          }
          targetExecute(target, sql, encoded)
          return []
        },
        catch: (cause) => sqlError(cause, "Failed to execute statement", "executeValues"),
      }),
    )

  const conn: Connection = {
    execute(sql, params, transformRows) {
      const eff = runQuery(sql, params, false) as Effect.Effect<
        ReadonlyArray<Record<string, unknown>>,
        SqlError
      >
      return transformRows
        ? (Effect.map(eff, transformRows as never) as Effect.Effect<
            ReadonlyArray<Record<string, unknown>>,
            SqlError
          >)
        : eff
    },
    executeRaw(sql, params) {
      return runQuery(sql, params, true) as Effect.Effect<unknown, SqlError>
    },
    executeValues(sql, params) {
      return runValues(sql, params)
    },
    executeUnprepared(sql, params, transformRows) {
      const eff = runQuery(sql, params ?? [], false) as Effect.Effect<
        ReadonlyArray<Record<string, unknown>>,
        SqlError
      >
      return transformRows
        ? (Effect.map(eff, transformRows as never) as Effect.Effect<
            ReadonlyArray<Record<string, unknown>>,
            SqlError
          >)
        : eff
    },
    executeStream(sql, params, transformRows) {
      // Stream.unwrap takes a `Effect<Stream, _, Scope>` and lifts the
      // scope into the stream's lifetime, so the permit is released on
      // stream completion AND on early interruption.
      return Stream.unwrap(
        Effect.gen(function* () {
          // Acquire the lock for the duration of the stream and
          // register the matching `release(1)` finalizer atomically
          // wrt interruption — otherwise an interrupt landing
          // between `take` and `addFinalizer` would leak the permit.
          yield* Effect.uninterruptibleMask((restore) =>
            Effect.gen(function* () {
              yield* restore(lock.take(1))
              yield* Effect.addFinalizer(() => lock.release(1))
            }),
          )
          const dbStream = yield* Effect.try({
            try: () => {
              const encoded = encodeAllParams(params)
              return targetQueryStream(target, sql, encoded)
            },
            catch: (cause) => sqlError(cause, "Failed to open result stream", "executeStream"),
          })
          const columns = yield* Effect.try({
            try: () => dbStream.getColumns(),
            catch: (cause) => sqlError(cause, "Failed to fetch stream columns", "executeStream"),
          })
          const stream: Stream.Stream<Record<string, unknown>, SqlError> = Stream.paginate(
            undefined as void,
            () =>
              Effect.try({
                try: () => {
                  const next = dbStream.getNext()
                  if (next === undefined) {
                    return [
                      [] as ReadonlyArray<Record<string, unknown>>,
                      Option.none<void>(),
                    ] as const
                  }
                  let decoded = decodeRows(next, columns, decodeTemporal)
                  if (transformRows) {
                    decoded = (
                      transformRows as (
                        rows: ReadonlyArray<Record<string, unknown>>,
                      ) => Array<Record<string, unknown>>
                    )(decoded)
                  }
                  return [
                    decoded as ReadonlyArray<Record<string, unknown>>,
                    Option.some<void>(undefined),
                  ] as const
                },
                catch: (cause) => sqlError(cause, "Failed to pull next batch", "executeStream"),
              }),
          )
          return stream
        }),
      )
    },
  }

  if (target._tag === "tx") {
    return Object.assign(conn, { [PgConnectionTxSymbol]: target }) as Connection
  }
  return conn
}

const txTarget = (conn: Connection): TxTarget | undefined =>
  (conn as unknown as Record<symbol, unknown>)[PgConnectionTxSymbol] as TxTarget | undefined

// ---------------------------------------------------------------------------
// makeImpl — open the connection, build the SqlClient
// ---------------------------------------------------------------------------

let pgClientIdCounter = 0

const makeImpl = (
  config: PgClientConfig,
): Effect.Effect<PgClient, SqlError, Scope.Scope | Reactivity.Reactivity | PostgresHostClient> =>
  Effect.gen(function* () {
    const decodeTemporal = config.decodeTemporal ?? "raw"

    const compiler = Statement.makeCompiler({
      dialect: "pg",
      placeholder: (i) => `$${i}`,
      onIdentifier: config.transformQueryNames
        ? (value, withoutTransform) =>
            withoutTransform ? escapePg(value) : escapePg(config.transformQueryNames!(value))
        : (value) => escapePg(value),
      onRecordUpdate: (placeholders, alias, columns, _values, returning) => {
        const sql = `(values ${placeholders}) AS ${alias}${columns}${
          returning ? ` RETURNING ${returning[0]}` : ""
        }`
        return [sql, returning ? returning[1] : []]
      },
      onCustom: () => ["", []],
    })

    const transformRows = config.transformResultNames
      ? Statement.defaultTransforms(config.transformResultNames).array
      : undefined

    const host = yield* PostgresHostClient
    const db = yield* Effect.try({
      try: () => host.open(config.connectionAddress),
      catch: (cause) =>
        sqlError(
          cause,
          `Failed to open postgres connection at ${config.connectionAddress}`,
          "open",
        ),
    })

    // Client-wide semaphore (1 permit) protecting the underlying
    // `DbConnection`. Any non-tx query takes/releases this; an outer
    // `withTransaction` holds it for the entire tx body.
    const clientLock = yield* Semaphore.make(1)

    const baseTarget: BaseTarget = { _tag: "base", db, clientLock }
    const baseConn = buildConnection(baseTarget, decodeTemporal)

    const acquirer: Acquirer = Effect.succeed(baseConn)

    const transactionService = Client.TransactionConnection(pgClientIdCounter++)

    const spanAttributes: ReadonlyArray<readonly [string, unknown]> = [
      ...(config.spanAttributes ? Object.entries(config.spanAttributes) : []),
      [ATTR_DB_SYSTEM_NAME, "postgresql"] as const,
    ]

    const baseClient = yield* Client.make({
      acquirer,
      compiler,
      transactionService,
      spanAttributes,
      transformRows,
    })

    // Acquire-connection helper used by makeWithTransaction. Holds the
    // client-wide lock for the whole tx body via the returned scope's
    // finalizer.
    // NOTE: ordering matters here. `clientLock.take(1)` is the only
    // step that must be interruptible (so a waiter can be cancelled),
    // but we MUST NOT register the `release(1)` finalizer until after
    // the take has actually succeeded — otherwise an interruption
    // while waiting would close the scope, release the permit we
    // never held, and over-inflate the semaphore.
    const acquireConnection: Effect.Effect<readonly [Scope.Closeable, Connection], SqlError> =
      Effect.uninterruptibleMask((restore) =>
        Effect.gen(function* () {
          const scope = yield* Scope.make()
          const acquired = yield* Effect.exit(restore(clientLock.take(1)))
          if (Exit.isFailure(acquired)) {
            yield* Scope.close(scope, Exit.void)
            return yield* Effect.failCause(acquired.cause)
          }
          yield* Scope.addFinalizer(scope, clientLock.release(1).pipe(Effect.asVoid))
          const txExit = yield* Effect.exit(
            Effect.try({
              try: () => db.beginTransaction(),
              catch: (cause) => sqlError(cause, "Failed to begin transaction", "beginTransaction"),
            }),
          )
          if (Exit.isFailure(txExit)) {
            yield* Scope.close(scope, Exit.void)
            return yield* Effect.failCause(txExit.cause)
          }
          const tx = txExit.value
          const txLock = yield* Semaphore.make(1)
          const target: TxTarget = { _tag: "tx", tx, txLock }
          const conn = buildConnection(target, decodeTemporal)
          return [scope, conn] as const
        }),
      )

    const begin = (_conn: Connection): Effect.Effect<void, SqlError> => Effect.void

    const commit = (conn: Connection): Effect.Effect<void, SqlError> => {
      const target = txTarget(conn)
      if (!target) return Effect.void
      return target.txLock.withPermits(1)(
        Effect.try({
          try: () => target.tx.commit(),
          catch: (cause) => sqlError(cause, "Failed to commit transaction", "commit"),
        }),
      )
    }

    const rollback = (conn: Connection): Effect.Effect<void, SqlError> => {
      const target = txTarget(conn)
      if (!target) return Effect.void
      return target.txLock.withPermits(1)(
        Effect.try({
          try: () => target.tx.rollback(),
          catch: (cause) => sqlError(cause, "Failed to rollback transaction", "rollback"),
        }),
      )
    }

    const savepoint = (conn: Connection, id: number): Effect.Effect<void, SqlError> => {
      const target = txTarget(conn)
      if (!target) return Effect.void
      return target.txLock.withPermits(1)(
        Effect.try({
          try: () => {
            target.tx.execute(`SAVEPOINT effect_sql_${id}`, [])
          },
          catch: (cause) => sqlError(cause, "Failed to create savepoint", "savepoint"),
        }),
      )
    }

    const rollbackSavepoint = (conn: Connection, id: number): Effect.Effect<void, SqlError> => {
      const target = txTarget(conn)
      if (!target) return Effect.void
      return target.txLock.withPermits(1)(
        Effect.try({
          try: () => {
            target.tx.execute(`ROLLBACK TO SAVEPOINT effect_sql_${id}`, [])
          },
          catch: (cause) => sqlError(cause, "Failed to rollback savepoint", "rollbackSavepoint"),
        }),
      )
    }

    const withTransaction = Client.makeWithTransaction({
      transactionService,
      spanAttributes,
      acquireConnection,
      begin,
      savepoint,
      commit,
      rollback,
      rollbackSavepoint,
    })

    const client: PgClient = Object.assign(baseClient, {
      [PgClientTypeId]: PgClientTypeId as PgClientTypeId,
      config,
      withTransaction,
    }) as unknown as PgClient

    return client
  })

const escapePg = (value: string): string => `"${value.replace(/"/g, '""')}"`

// ---------------------------------------------------------------------------
// Public factories
// ---------------------------------------------------------------------------

const make = (
  config: PgClientConfig,
): Effect.Effect<PgClient, SqlError, Scope.Scope | PostgresHostClient> =>
  Effect.provide(makeImpl(config), Reactivity.layer)

const layer = (
  config: PgClientConfig,
): Layer.Layer<PgClientService | Client.SqlClient, SqlError, PostgresHostClient> =>
  Layer.effectContext(
    Effect.map(makeImpl(config), (client) =>
      Context.make(PgClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide(Reactivity.layer))

/**
 * Public namespace mirror used by `import { PgClient } from "effect-golem/postgres"`.
 *
 * @since 0.1.0
 * @category constructors
 */
export const PgClient = {
  TypeId: PgClientTypeId,
  make,
  layer,
  PgClient: PgClientService,
}

/**
 * Probe an arbitrary value for the PgClient brand.
 *
 * @since 0.1.0
 * @category guards
 */
export const isPgClient = (v: unknown): v is PgClient => {
  if (v === null) return false
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (v as { readonly [PgClientTypeId]?: unknown })[PgClientTypeId] === PgClientTypeId
}
