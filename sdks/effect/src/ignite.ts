/**
 * Ignite (Apache Ignite 2.x) adapter for `effect-golem` agents —
 * exposes the official `effect/unstable/sql/SqlClient` interface on
 * top of Golem's `golem:rdbms/ignite2@1.5.0` host bindings.
 *
 * Consumed via the `effect-golem/ignite2` sub-import. Inside the
 * Golem `wasm-rquickjs` runtime the bindings come from the embedded
 * base WASM; for Node tests they are aliased to in-memory fakes via
 * `vitest.config.ts`.
 *
 * Highlights:
 * - Compiler dialect: `"sqlite"` (Ignite SQL is closest to ANSI; we
 *   pick the SQLite dialect's `?` placeholders and `"`-quoted
 *   identifiers since Ignite SQL accepts both).
 * - Transaction handling: built on `Client.make`, with a custom
 *   `Client.makeWithTransaction` wired directly to
 *   `DbConnection.beginTransaction()` / `tx.commit()` /
 *   `tx.rollback()`. **Nested transactions are explicitly rejected**
 *   with a `SqlSyntaxError` because Apache Ignite does not support
 *   savepoints — there is no way to make `withTransaction` re-entrant
 *   safely.
 * - Two locks: a client-wide semaphore guarding the underlying
 *   `DbConnection`, and a per-`DbTransaction` semaphore so concurrent
 *   fibers inside a tx serialise on the resource.
 * - Explicit param encoding: rich Ignite types (uuid, decimal, date,
 *   timestamp, time, char, byte-array) require an explicit
 *   `Ignite.<helper>(...)` call. Plain JS values are mapped
 *   conservatively (string→db-string, integer→db-int or db-long,
 *   finite float→db-double, bigint→db-long, boolean→db-boolean,
 *   Date→db-date (epoch millis), Uint8Array→db-byte-array,
 *   null/undefined→db-null). NaN / ±Infinity are rejected.
 * - Conservative row decoding: numeric scalars unwrap to JS values,
 *   `db-uuid` `[bigint, bigint]` decode to canonical 36-char strings,
 *   bytes stay as `Uint8Array`. Temporal values stay as raw bigints
 *   unless `decodeTemporal: "date"` is set, in which case `db-date`
 *   and `db-timestamp` decode to JS `Date` (UTC).
 * - Streaming: `executeStream` opens a `DbResultStream`, holds the
 *   per-connection lock for its lifetime, and uses `Stream.unwrap`
 *   over a scoped effect so the permit is released on stream
 *   completion AND on early interruption.
 *
 * Resource lifecycle: the WIT host bindings only expose synchronous
 * `Db*` resource constructors / methods — there is no public `close`
 * on `DbConnection`. The adapter relies on the host's GC to free
 * resources once the JS handle becomes unreachable.
 *
 * Note: the `golem:rdbms/ignite2@1.5.0` host binding may be missing
 * in some Golem deployments; the integration-test agent for Ignite
 * is therefore deployed as a separate, gated component.
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
import { SqlError, SqlSyntaxError } from "effect/unstable/sql/SqlError"
import * as Statement from "effect/unstable/sql/Statement"
import {
  DbConnection,
  type DbColumn,
  type DbResultStream,
  type DbRow,
  type DbTransaction,
  type DbValue,
} from "golem:rdbms/ignite2@1.5.0"
import { ParamEncodingError, READ_PREFIX_RE, sqlErrorFor, toBigIntChecked } from "./rdbms-shared.js"

const ATTR_DB_SYSTEM_NAME = "db.system.name"

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * Unique symbol stamped on every {@link IgniteClient} instance so
 * consumers can reliably distinguish an Ignite client. Keyed via
 * `Symbol.for` so multiple module copies still agree on the same key.
 */
export const IgniteClientTypeId: unique symbol = Symbol.for(
  "effect-golem/IgniteClient",
) as IgniteClientTypeId
export type IgniteClientTypeId = typeof IgniteClientTypeId

const IgniteConnectionTxSymbol: unique symbol = Symbol.for(
  "effect-golem/IgniteClient/__txContext",
) as typeof IgniteConnectionTxSymbol

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** How temporal values (db-date / db-timestamp) are decoded from rows. */
export type TemporalDecodeMode = "raw" | "date"

/** Configuration accepted by {@link IgniteClient.make} / {@link IgniteClient.layer}. */
export interface IgniteClientConfig {
  /**
   * Ignite connection address — e.g.
   * `ignite://[user:pass@]host:port[?pool_size=N&tls=true]`. Default
   * port: 10800. Passed verbatim to `DbConnection.open(...)`.
   */
  readonly connectionAddress: string
  readonly transformResultNames?: ((str: string) => string) | undefined
  readonly transformQueryNames?: ((str: string) => string) | undefined
  /**
   * How to decode `db-date` (millis since Unix epoch UTC) and
   * `db-timestamp` (millis + sub-millisecond nanos) values from query
   * rows.
   *
   * - `"raw"` (default): keep the raw bigint / `[bigint, number]` as-is.
   * - `"date"`: decode to JS `Date` (UTC).
   */
  readonly decodeTemporal?: TemporalDecodeMode | undefined
  /** Span attributes (in addition to the auto-injected `db.system.name`). */
  readonly spanAttributes?: Record<string, unknown> | undefined
  /** Reserved for future use. */
  readonly prepareCacheSize?: number | undefined
  readonly prepareCacheTTL?: Duration.Input | undefined
}

/**
 * The public IgniteClient — extends `effect/unstable/sql/SqlClient`
 * so users can use `SqlSchema` / `SqlResolver` / `Migrator` and
 * resolve the canonical `Client.SqlClient` tag.
 *
 * Note: `withTransaction` rejects nested calls with a
 * `SqlSyntaxError` because Ignite does not support savepoints.
 */
export interface IgniteClient extends Client.SqlClient {
  readonly [IgniteClientTypeId]: IgniteClientTypeId
  readonly config: IgniteClientConfig
  readonly updateValues: never
}

// ---------------------------------------------------------------------------
// Effect Context tag
// ---------------------------------------------------------------------------

/**
 * Context tag for resolving an {@link IgniteClient} from the
 * environment. Both this tag and the upstream `Client.SqlClient` tag
 * are populated by {@link IgniteClient.layer}.
 */
export class IgniteClientService extends Context.Service<IgniteClientService, IgniteClient>()(
  "effect-golem/IgniteClient",
) {}

// ---------------------------------------------------------------------------
// Ignite helpers — explicit sentinel wrappers used in tagged-template params
// ---------------------------------------------------------------------------

const IgniteParamTag: unique symbol = Symbol.for(
  "effect-golem/IgniteClient/__param",
) as typeof IgniteParamTag

interface IgniteParam<T extends string, V> {
  readonly [IgniteParamTag]: true
  readonly kind: T
  readonly value: V
}

const igniteParam = <T extends string, V>(kind: T, value: V): IgniteParam<T, V> => ({
  [IgniteParamTag]: true,
  kind,
  value,
})

const isIgniteParam = (v: unknown): v is IgniteParam<string, unknown> =>
  typeof v === "object" && v !== null && (v as Record<symbol, unknown>)[IgniteParamTag] === true

/** Ignite uuid — `[hi, lo]` 128-bit identifier. */
export type IgniteUuid = string | { readonly hi: bigint; readonly lo: bigint } | [bigint, bigint]

/**
 * Explicit parameter wrappers for rich Ignite types.
 *
 * ```ts
 * yield* sql`INSERT INTO t (id, ts) VALUES (${Ignite.uuid(id)}, ${Ignite.timestamp(BigInt(Date.now()), 0)})`
 * ```
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

// ---------------------------------------------------------------------------
// Param encoding — JS / Ignite helpers → DbValue
// ---------------------------------------------------------------------------

const NULL_DB_VALUE: DbValue = { tag: "db-null" }

const I32_MIN = -2_147_483_648
const I32_MAX = 2_147_483_647
const I64_MIN = BigInt("-9223372036854775808")
const I64_MAX = BigInt("9223372036854775807")
const U64_MAX = BigInt("18446744073709551615")

const isSafeInt32 = (n: number): boolean => Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX

const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of db-long range`)
  }
}

const checkUint64 = (b: bigint, label: string): void => {
  if (b < 0n || b > U64_MAX) {
    throw new ParamEncodingError(`${label} ${b.toString()} is out of u64 range`)
  }
}

const encodeUuid = (input: IgniteUuid): [bigint, bigint] => {
  if (typeof input === "string") {
    const normalized = input.replace(/-/g, "").toLowerCase()
    if (!/^[0-9a-f]{32}$/.test(normalized)) {
      throw new ParamEncodingError(`invalid uuid: ${input}`)
    }
    const hi = BigInt("0x" + normalized.slice(0, 16))
    const lo = BigInt("0x" + normalized.slice(16, 32))
    return [hi, lo]
  }
  // WIT spec: db-uuid is `tuple<u64, u64>`. Validate both halves.
  const hi = Array.isArray(input) ? input[0] : input.hi
  const lo = Array.isArray(input) ? input[1] : input.lo
  if (typeof hi !== "bigint" || typeof lo !== "bigint") {
    throw new ParamEncodingError("Ignite.uuid hi/lo must both be bigint")
  }
  checkUint64(hi, "Ignite.uuid hi")
  checkUint64(lo, "Ignite.uuid lo")
  return [hi, lo]
}

const dateToEpochMillis = (d: Date): bigint => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return BigInt(ms)
}

/**
 * Convert a single template-literal parameter to a `DbValue`.
 */
const encodeDbValue = (value: unknown): DbValue => {
  if (value === null || value === undefined) return NULL_DB_VALUE
  if (isIgniteParam(value)) {
    return encodeIgniteParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "db-string", val: value }
    case "boolean":
      return { tag: "db-boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "db-long", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to Ignite; use Ignite.decimal(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "db-int", val: value }
      }
      if (Number.isInteger(value)) {
        return { tag: "db-long", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "db-double", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "db-byte-array", val: value }
  }
  if (value instanceof Date) {
    return { tag: "db-date", val: dateToEpochMillis(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use an Ignite.<helper>(...) wrapper`,
  )
}

const encodeIgniteParam = (param: IgniteParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "uuid":
      return { tag: "db-uuid", val: encodeUuid(param.value as IgniteUuid) }
    case "decimal":
      return { tag: "db-decimal", val: param.value as string }
    case "date": {
      const v = param.value as bigint | number | Date
      const millis =
        v instanceof Date ? dateToEpochMillis(v) : toBigIntChecked(v, "Ignite.date millis")
      return { tag: "db-date", val: millis }
    }
    case "timestamp": {
      const { millis, subMilliNanos } = param.value as {
        millis: bigint | number
        subMilliNanos: number
      }
      const m = toBigIntChecked(millis, "Ignite.timestamp millis")
      if (!Number.isInteger(subMilliNanos) || subMilliNanos < 0 || subMilliNanos > 999_999) {
        throw new ParamEncodingError(
          `Ignite.timestamp sub-ms-nanos must be an integer in 0..999_999; got ${String(subMilliNanos)}`,
        )
      }
      return { tag: "db-timestamp", val: [m, subMilliNanos] }
    }
    case "time": {
      const v = param.value as bigint | number
      return { tag: "db-time", val: toBigIntChecked(v, "Ignite.time nanos") }
    }
    case "char":
      return { tag: "db-char", val: param.value as number }
    case "byte-array":
      return { tag: "db-byte-array", val: param.value as Uint8Array }
    case "byte":
      return { tag: "db-byte", val: param.value as number }
    case "short":
      return { tag: "db-short", val: param.value as number }
    case "int":
      return { tag: "db-int", val: param.value as number }
    case "long": {
      const b = toBigIntChecked(param.value as number | bigint, "Ignite.long")
      checkInt64(b)
      return { tag: "db-long", val: b }
    }
    case "float":
      return { tag: "db-float", val: param.value as number }
    case "double":
      return { tag: "db-double", val: param.value as number }
    case "string":
      return { tag: "db-string", val: param.value as string }
    case "boolean":
      return { tag: "db-boolean", val: param.value as boolean }
    default:
      throw new ParamEncodingError(`unknown Ignite helper: ${param.kind}`)
  }
}

const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

const uuidToString = (u: [bigint, bigint]): string => {
  const hi = u[0].toString(16).padStart(16, "0")
  const lo = u[1].toString(16).padStart(16, "0")
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`
}

const decodeDbValue = (value: DbValue, decodeTemporal: TemporalDecodeMode): unknown => {
  switch (value.tag) {
    case "db-null":
      return null
    case "db-boolean":
      return value.val
    case "db-byte":
    case "db-short":
    case "db-int":
    case "db-float":
    case "db-double":
    case "db-char":
      return value.val
    case "db-long":
      return value.val
    case "db-string":
    case "db-decimal":
      return value.val
    case "db-uuid":
      return uuidToString(value.val)
    case "db-date":
      return decodeTemporal === "date" ? new Date(Number(value.val)) : value.val
    case "db-timestamp": {
      if (decodeTemporal !== "date") return value.val
      const [millis] = value.val
      return new Date(Number(millis))
    }
    case "db-time":
      return value.val
    case "db-byte-array":
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

// Ignite SQL has no `RETURNING` clause; only the read-prefix matters.
const isReader = (sql: string): boolean => READ_PREFIX_RE.test(sql)

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
    return Object.assign(conn, { [IgniteConnectionTxSymbol]: target }) as Connection
  }
  return conn
}

const txTarget = (conn: Connection): TxTarget | undefined =>
  (conn as unknown as Record<symbol, unknown>)[IgniteConnectionTxSymbol] as TxTarget | undefined

// ---------------------------------------------------------------------------
// makeImpl — open the connection, build the SqlClient
// ---------------------------------------------------------------------------

let igniteClientIdCounter = 0

const makeImpl = (
  config: IgniteClientConfig,
): Effect.Effect<IgniteClient, SqlError, Scope.Scope | Reactivity.Reactivity> =>
  Effect.gen(function* () {
    const decodeTemporal = config.decodeTemporal ?? "raw"

    const compiler = Statement.makeCompiler({
      // No native dialect for Ignite; use SQLite which closely matches
      // (`?` placeholders, double-quoted identifiers).
      dialect: "sqlite",
      placeholder: () => `?`,
      onIdentifier: config.transformQueryNames
        ? (value, withoutTransform) =>
            withoutTransform
              ? escapeIgnite(value)
              : escapeIgnite(config.transformQueryNames!(value))
        : (value) => escapeIgnite(value),
      onRecordUpdate: () => ["", []],
      onCustom: () => ["", []],
    })

    const transformRows = config.transformResultNames
      ? Statement.defaultTransforms(config.transformResultNames).array
      : undefined

    const db = yield* Effect.try({
      try: () => DbConnection.open(config.connectionAddress),
      catch: (cause) =>
        sqlError(cause, `Failed to open Ignite connection at ${config.connectionAddress}`, "open"),
    })

    const clientLock = yield* Semaphore.make(1)

    const baseTarget: BaseTarget = { _tag: "base", db, clientLock }
    const baseConn = buildConnection(baseTarget, decodeTemporal)

    const acquirer: Acquirer = Effect.succeed(baseConn)

    const transactionService = Client.TransactionConnection(igniteClientIdCounter++)

    const spanAttributes: ReadonlyArray<readonly [string, unknown]> = [
      ...(config.spanAttributes ? Object.entries(config.spanAttributes) : []),
      [ATTR_DB_SYSTEM_NAME, "ignite"] as const,
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

    // Apache Ignite does not support savepoints — nested
    // `withTransaction` calls cannot work safely, so reject them
    // with a synthetic SqlSyntaxError instead of silently issuing a
    // SAVEPOINT statement that the host would reject anyway.
    const nestedTxError = (operation: string): SqlError =>
      new SqlError({
        reason: new SqlSyntaxError({
          cause: new Error(
            "nested withTransaction is not supported on Ignite — savepoints are unavailable",
          ),
          message: "nested withTransaction is not supported on Ignite — savepoints are unavailable",
          operation,
        }),
      })

    const savepoint = (_conn: Connection, _id: number): Effect.Effect<void, SqlError> =>
      Effect.fail(nestedTxError("savepoint"))

    // No-op: `savepoint(...)` always fails before any savepoint is
    // actually issued, so there is never anything to roll back. If we
    // also failed here, `Client.makeWithTransaction` would `orDie`
    // this rollback failure and swallow the original `SqlSyntaxError`
    // cause.
    const rollbackSavepoint = (_conn: Connection, _id: number): Effect.Effect<void, SqlError> =>
      Effect.void

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

    const client: IgniteClient = Object.assign(baseClient, {
      [IgniteClientTypeId]: IgniteClientTypeId as IgniteClientTypeId,
      config,
      withTransaction,
    }) as unknown as IgniteClient

    return client
  })

const escapeIgnite = (value: string): string => `"${value.replace(/"/g, '""')}"`

// ---------------------------------------------------------------------------
// Public factories
// ---------------------------------------------------------------------------

const make = (config: IgniteClientConfig): Effect.Effect<IgniteClient, SqlError, Scope.Scope> =>
  Effect.provide(makeImpl(config), Reactivity.layer)

const layer = (
  config: IgniteClientConfig,
): Layer.Layer<IgniteClientService | Client.SqlClient, SqlError> =>
  Layer.effectContext(
    Effect.map(makeImpl(config), (client) =>
      Context.make(IgniteClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide(Reactivity.layer))

/**
 * Public namespace mirror used by `import { IgniteClient } from "effect-golem/ignite2"`.
 */
export const IgniteClient = {
  TypeId: IgniteClientTypeId,
  make,
  layer,
  IgniteClient: IgniteClientService,
}

/** Probe an arbitrary value for the IgniteClient brand. */
export const isIgniteClient = (v: unknown): v is IgniteClient => {
  if (v === null) return false
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (
    (v as { readonly [IgniteClientTypeId]?: unknown })[IgniteClientTypeId] === IgniteClientTypeId
  )
}
