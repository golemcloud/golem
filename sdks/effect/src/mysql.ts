/**
 * MySQL adapter for `effect-golem` agents — exposes the official
 * `effect/unstable/sql/SqlClient` interface on top of Golem's
 * `golem:rdbms/mysql@1.5.0` host bindings.
 *
 * Consumed via the `effect-golem/mysql` sub-import. Inside the Golem
 * `wasm-rquickjs` runtime the bindings come from the embedded base
 * WASM; for Node tests they are aliased to in-memory fakes via
 * `vitest.config.ts`.
 *
 * Why not `@effect/sql-mysql2` / `@effect/sql-mysql`? Those packages
 * depend on native drivers (e.g. `mysql2`) that cannot run inside
 * `wasm-rquickjs`. This module mirrors the Postgres adapter
 * (`src/postgres.ts`) so users get the same `SqlSchema` / `SqlResolver`
 * / `Migrator` integration but the queries flow through the host's
 * `DbConnection` / `DbTransaction` resources.
 *
 * Highlights:
 * - Compiler dialect: `"mysql"` (positional `?` placeholders,
 *   backtick-quoted identifiers).
 * - Transaction handling: built on `Client.make`, with a custom
 *   `Client.makeWithTransaction` wired directly to
 *   `DbConnection.beginTransaction()` / `tx.commit()` /
 *   `tx.rollback()`. Nested calls use `SAVEPOINT effect_sql_<id>`
 *   issued via the same `DbTransaction` resource — MySQL 5.7+ /
 *   MariaDB support savepoints natively.
 * - Two locks: a client-wide semaphore guarding the underlying
 *   `DbConnection`, and a per-`DbTransaction` semaphore so concurrent
 *   fibers inside a tx serialise on the resource.
 * - Explicit param encoding: rich MySQL types (json, decimal, date,
 *   time, datetime, set, enumeration, bit, blob/binary variants)
 *   require an explicit `MySql.<helper>(...)` call. Plain JS values
 *   are mapped conservatively (string→varchar, integer→int or bigint,
 *   finite float→double, bigint→bigint, boolean, Date→datetime,
 *   Uint8Array→blob, null/undefined→null). NaN / ±Infinity are
 *   rejected.
 * - Conservative row decoding: numeric scalars unwrap to JS values,
 *   bytes stay as `Uint8Array`, JSON stays as a string. Temporal
 *   values stay as raw structs unless `decodeTemporal: "date"` is
 *   set, in which case `datetime` / `timestamp` decode to JS `Date`
 *   (UTC).
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
 * resources once the JS handle becomes unreachable.
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
  type Date as MyDate,
  DbConnection,
  type DbColumn,
  type DbResultStream,
  type DbRow,
  type DbTransaction,
  type DbValue,
  type Time,
  type Timestamp,
} from "golem:rdbms/mysql@1.5.0"
import {
  ParamEncodingError,
  READ_PREFIX_RE,
  RETURNING_RE,
  sqlErrorFor,
  toBigIntChecked,
} from "./rdbms-shared.js"

const ATTR_DB_SYSTEM_NAME = "db.system.name"

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * Unique symbol stamped on every {@link MySqlClient} instance so
 * consumers can reliably distinguish a MySQL client. Keyed via
 * `Symbol.for` so multiple module copies (e.g. one bundled inside
 * `effect-golem` and one in the standalone `effect-golem/mysql`
 * sub-import) still agree on the same key.
 */
export const MySqlClientTypeId: unique symbol = Symbol.for(
  "effect-golem/MySqlClient",
) as MySqlClientTypeId
export type MySqlClientTypeId = typeof MySqlClientTypeId

const MySqlConnectionTxSymbol: unique symbol = Symbol.for(
  "effect-golem/MySqlClient/__txContext",
) as typeof MySqlConnectionTxSymbol

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** How temporal values (datetime/timestamp/date/time) are decoded from rows. */
export type TemporalDecodeMode = "raw" | "date"

/**
 * Configuration accepted by {@link MySqlClient.make} /
 * {@link MySqlClient.layer}.
 */
export interface MySqlClientConfig {
  /**
   * MySQL connection address — e.g.
   * `mysql://user:pass@host:3306/dbname`. Passed verbatim to
   * `DbConnection.open(...)`.
   */
  readonly connectionAddress: string
  readonly transformResultNames?: ((str: string) => string) | undefined
  readonly transformQueryNames?: ((str: string) => string) | undefined
  /**
   * How to decode `datetime`, `timestamp`, `date`, `time` values from
   * query rows.
   *
   * - `"raw"` (default): keep the raw struct from the host as-is.
   * - `"date"`: decode `datetime` / `timestamp` to JS `Date` (UTC).
   *   `date` becomes a `Date` at UTC midnight; `time` stays raw
   *   because JS `Date` cannot represent it faithfully.
   */
  readonly decodeTemporal?: TemporalDecodeMode | undefined
  /** Span attributes (in addition to the auto-injected `db.system.name`). */
  readonly spanAttributes?: Record<string, unknown> | undefined
  /** Reserved for future use. */
  readonly prepareCacheSize?: number | undefined
  readonly prepareCacheTTL?: Duration.Input | undefined
}

/**
 * The public MySqlClient — extends the official
 * `effect/unstable/sql/SqlClient` so users can write
 * `yield* sql\`SELECT ...\`` queries, compose with `SqlSchema` /
 * `SqlResolver` / `Migrator`, and resolve the canonical
 * `Client.SqlClient` tag.
 */
export interface MySqlClient extends Client.SqlClient {
  readonly [MySqlClientTypeId]: MySqlClientTypeId
  readonly config: MySqlClientConfig
  /** Multi-row update isn't surfaced — matches @effect/sql-pg's surface. */
  readonly updateValues: never
}

// ---------------------------------------------------------------------------
// Effect Context tag
// ---------------------------------------------------------------------------

/**
 * Context tag for resolving a {@link MySqlClient} from the environment.
 * Both this tag and the upstream `Client.SqlClient` tag are populated
 * by {@link MySqlClient.layer}.
 */
export class MySqlClientService extends Context.Service<MySqlClientService, MySqlClient>()(
  "effect-golem/MySqlClient",
) {}

// ---------------------------------------------------------------------------
// MySql helpers — explicit sentinel wrappers used in tagged-template params
// ---------------------------------------------------------------------------

const MySqlParamTag: unique symbol = Symbol.for(
  "effect-golem/MySqlClient/__param",
) as typeof MySqlParamTag

interface MySqlParam<T extends string, V> {
  readonly [MySqlParamTag]: true
  readonly kind: T
  readonly value: V
}

const mysqlParam = <T extends string, V>(kind: T, value: V): MySqlParam<T, V> => ({
  [MySqlParamTag]: true,
  kind,
  value,
})

const isMySqlParam = (v: unknown): v is MySqlParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[MySqlParamTag] === true

/**
 * Explicit parameter wrappers for rich MySQL types. Use inside
 * tagged-template literals to override the conservative default
 * mapping:
 *
 * ```ts
 * yield* sql`INSERT INTO t (id, data) VALUES (${id}, ${MySql.json({ foo: 1 })})`
 * ```
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

// ---------------------------------------------------------------------------
// Param encoding — JS / MySql helpers → DbValue
// ---------------------------------------------------------------------------

const NULL_DB_VALUE: DbValue = { tag: "null" }

const I32_MIN = -2_147_483_648
const I32_MAX = 2_147_483_647
const I64_MIN = BigInt("-9223372036854775808")
const I64_MAX = BigInt("9223372036854775807")
const U64_MAX = BigInt("18446744073709551615")

const isSafeInt32 = (n: number): boolean => Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX

const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of bigint range`)
  }
}

const checkUint64 = (b: bigint): void => {
  if (b < 0n || b > U64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of bigint-unsigned range`)
  }
}

const dateToTimestamp = (d: Date): Timestamp => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return {
    date: { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() },
    time: {
      hour: d.getUTCHours(),
      minute: d.getUTCMinutes(),
      second: d.getUTCSeconds(),
      nanosecond: d.getUTCMilliseconds() * 1_000_000,
    },
  }
}

/**
 * Convert a single template-literal parameter to a `DbValue`. Throws
 * a `ParamEncodingError` for unsupported shapes; the caller wraps it
 * into a `SqlError`.
 */
const encodeDbValue = (value: unknown): DbValue => {
  if (value === null || value === undefined) return NULL_DB_VALUE
  if (isMySqlParam(value)) {
    return encodeMySqlParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "varchar", val: value }
    case "boolean":
      return { tag: "boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "bigint", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to MySQL; use MySql.decimal(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "int", val: value }
      }
      if (Number.isInteger(value)) {
        // `Number.isInteger` is true even for non-safe-integer values
        // — `toBigIntChecked` rejects those with a typed
        // `ParamEncodingError` instead of silently dropping precision.
        return { tag: "bigint", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "double", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "blob", val: value }
  }
  if (value instanceof Date) {
    return { tag: "datetime", val: dateToTimestamp(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a MySql.<helper>(...) wrapper`,
  )
}

const encodeMySqlParam = (param: MySqlParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "json":
      return { tag: "json", val: JSON.stringify(param.value) }
    case "enumeration":
      return { tag: "enumeration", val: param.value as string }
    case "set":
      return { tag: "set", val: param.value as string }
    case "bit":
      return { tag: "bit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "decimal":
      return { tag: "decimal", val: param.value as string }
    case "year":
      return { tag: "year", val: param.value as number }
    case "tinyint":
      return { tag: "tinyint", val: param.value as number }
    case "smallint":
      return { tag: "smallint", val: param.value as number }
    case "mediumint":
      return { tag: "mediumint", val: param.value as number }
    case "int":
      return { tag: "int", val: param.value as number }
    case "bigint": {
      const b = toBigIntChecked(param.value as number | bigint, "MySql.bigint")
      checkInt64(b)
      return { tag: "bigint", val: b }
    }
    case "tinyint-unsigned":
      return { tag: "tinyint-unsigned", val: param.value as number }
    case "smallint-unsigned":
      return { tag: "smallint-unsigned", val: param.value as number }
    case "mediumint-unsigned":
      return { tag: "mediumint-unsigned", val: param.value as number }
    case "int-unsigned":
      return { tag: "int-unsigned", val: param.value as number }
    case "bigint-unsigned": {
      const b = toBigIntChecked(param.value as number | bigint, "MySql.bigintUnsigned")
      checkUint64(b)
      return { tag: "bigint-unsigned", val: b }
    }
    case "float":
      return { tag: "float", val: param.value as number }
    case "double":
      return { tag: "double", val: param.value as number }
    case "fixchar":
      return { tag: "fixchar", val: param.value as string }
    case "varchar":
      return { tag: "varchar", val: param.value as string }
    case "tinytext":
      return { tag: "tinytext", val: param.value as string }
    case "text":
      return { tag: "text", val: param.value as string }
    case "mediumtext":
      return { tag: "mediumtext", val: param.value as string }
    case "longtext":
      return { tag: "longtext", val: param.value as string }
    case "binary":
      return { tag: "binary", val: param.value as Uint8Array }
    case "varbinary":
      return { tag: "varbinary", val: param.value as Uint8Array }
    case "tinyblob":
      return { tag: "tinyblob", val: param.value as Uint8Array }
    case "blob":
      return { tag: "blob", val: param.value as Uint8Array }
    case "mediumblob":
      return { tag: "mediumblob", val: param.value as Uint8Array }
    case "longblob":
      return { tag: "longblob", val: param.value as Uint8Array }
    case "date":
      return { tag: "date", val: param.value as MyDate }
    case "time":
      return { tag: "time", val: param.value as Time }
    case "datetime":
      return { tag: "datetime", val: param.value as Timestamp }
    case "timestamp":
      return { tag: "timestamp", val: param.value as Timestamp }
    default:
      throw new ParamEncodingError(`unknown MySql helper: ${param.kind}`)
  }
}

const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

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

const dateOnlyToDate = (d: { year: number; month: number; day: number }): Date =>
  new Date(Date.UTC(d.year, d.month - 1, d.day))

const decodeDbValue = (value: DbValue, decodeTemporal: TemporalDecodeMode): unknown => {
  switch (value.tag) {
    case "null":
      return null
    case "boolean":
      return value.val
    case "tinyint":
    case "smallint":
    case "mediumint":
    case "int":
    case "tinyint-unsigned":
    case "smallint-unsigned":
    case "mediumint-unsigned":
    case "int-unsigned":
    case "year":
    case "float":
    case "double":
      return value.val
    case "bigint":
    case "bigint-unsigned":
      return value.val
    case "decimal":
      return value.val
    case "fixchar":
    case "varchar":
    case "tinytext":
    case "text":
    case "mediumtext":
    case "longtext":
    case "json":
    case "enumeration":
    case "set":
      return value.val
    case "binary":
    case "varbinary":
    case "tinyblob":
    case "blob":
    case "mediumblob":
    case "longblob":
      return value.val
    case "bit":
      return value.val
    case "datetime":
    case "timestamp":
      return decodeTemporal === "date" ? timestampToDate(value.val) : value.val
    case "date":
      return decodeTemporal === "date" ? dateOnlyToDate(value.val) : value.val
    case "time":
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
      return Stream.unwrap(
        Effect.gen(function* () {
          // Atomic lock-acquire + release-finalizer so an interrupt
          // landing between `take` and `addFinalizer` cannot leak
          // the permit. See postgres.ts for the same pattern.
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
    return Object.assign(conn, { [MySqlConnectionTxSymbol]: target }) as Connection
  }
  return conn
}

const txTarget = (conn: Connection): TxTarget | undefined =>
  (conn as unknown as Record<symbol, unknown>)[MySqlConnectionTxSymbol] as TxTarget | undefined

// ---------------------------------------------------------------------------
// makeImpl — open the connection, build the SqlClient
// ---------------------------------------------------------------------------

let mysqlClientIdCounter = 0

const makeImpl = (
  config: MySqlClientConfig,
): Effect.Effect<MySqlClient, SqlError, Scope.Scope | Reactivity.Reactivity> =>
  Effect.gen(function* () {
    const decodeTemporal = config.decodeTemporal ?? "raw"

    const compiler = Statement.makeCompiler({
      dialect: "mysql",
      placeholder: () => `?`,
      onIdentifier: config.transformQueryNames
        ? (value, withoutTransform) =>
            withoutTransform ? escapeMySql(value) : escapeMySql(config.transformQueryNames!(value))
        : (value) => escapeMySql(value),
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

    const db = yield* Effect.try({
      try: () => DbConnection.open(config.connectionAddress),
      catch: (cause) =>
        sqlError(cause, `Failed to open MySQL connection at ${config.connectionAddress}`, "open"),
    })

    const clientLock = yield* Semaphore.make(1)

    const baseTarget: BaseTarget = { _tag: "base", db, clientLock }
    const baseConn = buildConnection(baseTarget, decodeTemporal)

    const acquirer: Acquirer = Effect.succeed(baseConn)

    const transactionService = Client.TransactionConnection(mysqlClientIdCounter++)

    const spanAttributes: ReadonlyArray<readonly [string, unknown]> = [
      ...(config.spanAttributes ? Object.entries(config.spanAttributes) : []),
      [ATTR_DB_SYSTEM_NAME, "mysql"] as const,
    ]

    const baseClient = yield* Client.make({
      acquirer,
      compiler,
      transactionService,
      spanAttributes,
      transformRows,
    })

    // NOTE: see postgres.ts for the rationale. We must not register
    // the `release(1)` finalizer before the `take(1)` actually
    // succeeds, otherwise an interrupted waiter would over-release
    // the semaphore.
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

    const client: MySqlClient = Object.assign(baseClient, {
      [MySqlClientTypeId]: MySqlClientTypeId as MySqlClientTypeId,
      config,
      withTransaction,
    }) as unknown as MySqlClient

    return client
  })

const escapeMySql = (value: string): string => `\`${value.replace(/`/g, "``")}\``

// ---------------------------------------------------------------------------
// Public factories
// ---------------------------------------------------------------------------

const make = (config: MySqlClientConfig): Effect.Effect<MySqlClient, SqlError, Scope.Scope> =>
  Effect.provide(makeImpl(config), Reactivity.layer)

const layer = (
  config: MySqlClientConfig,
): Layer.Layer<MySqlClientService | Client.SqlClient, SqlError> =>
  Layer.effectContext(
    Effect.map(makeImpl(config), (client) =>
      Context.make(MySqlClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide(Reactivity.layer))

/**
 * Public namespace mirror used by `import { MySqlClient } from "effect-golem/mysql"`.
 */
export const MySqlClient = {
  TypeId: MySqlClientTypeId,
  make,
  layer,
  MySqlClient: MySqlClientService,
}

/** Probe an arbitrary value for the MySqlClient brand. */
export const isMySqlClient = (v: unknown): v is MySqlClient => {
  if (v === null) return false
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (v as { readonly [MySqlClientTypeId]?: unknown })[MySqlClientTypeId] === MySqlClientTypeId
}
