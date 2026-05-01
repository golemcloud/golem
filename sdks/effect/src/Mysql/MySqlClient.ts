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
  type DbConnection,
  type DbResultStream,
  type DbTransaction,
  type DbValue,
} from "golem:rdbms/mysql@1.5.0"
import { MysqlHostClient } from "../host/MysqlHostClient.js"
import { sqlErrorFor, READ_PREFIX_RE, RETURNING_RE } from "../internal/rdbmsShared.js"
import { decodeRows, decodeRowsValues, encodeAllParams } from "./internal/codec.js"

export { MySql, isMySqlParam, MySqlParamTag, type MySqlParam } from "./MySql.js"

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
 *
 * @since 0.1.0
 * @category symbols
 */
export const MySqlClientTypeId: unique symbol = Symbol.for(
  "effect-golem/MySqlClient",
) as MySqlClientTypeId

/**
 * @since 0.1.0
 * @category symbols
 */
export type MySqlClientTypeId = typeof MySqlClientTypeId

const MySqlConnectionTxSymbol: unique symbol = Symbol.for(
  "effect-golem/MySqlClient/__txContext",
) as typeof MySqlConnectionTxSymbol

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * How temporal values (datetime/timestamp/date/time) are decoded from rows.
 *
 * @since 0.1.0
 * @category models
 */
export type TemporalDecodeMode = "raw" | "date"

/**
 * Configuration accepted by {@link MySqlClient.make} /
 * {@link MySqlClient.layer}.
 *
 * @since 0.1.0
 * @category models
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
 *
 * @since 0.1.0
 * @category models
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
 *
 * @since 0.1.0
 * @category host services
 */
export class MySqlClientService extends Context.Service<MySqlClientService, MySqlClient>()(
  "effect-golem/MySqlClient",
) {}

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
): Effect.Effect<MySqlClient, SqlError, Scope.Scope | Reactivity.Reactivity | MysqlHostClient> =>
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

    const host = yield* MysqlHostClient
    const db = yield* Effect.try({
      try: () => host.open(config.connectionAddress),
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

const make = (
  config: MySqlClientConfig,
): Effect.Effect<MySqlClient, SqlError, Scope.Scope | MysqlHostClient> =>
  Effect.provide(makeImpl(config), Reactivity.layer)

const layer = (
  config: MySqlClientConfig,
): Layer.Layer<MySqlClientService | Client.SqlClient, SqlError, MysqlHostClient> =>
  Layer.effectContext(
    Effect.map(makeImpl(config), (client) =>
      Context.make(MySqlClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide(Reactivity.layer))

/**
 * Public namespace mirror used by `import { MySqlClient } from "effect-golem/mysql"`.
 *
 * @since 0.1.0
 * @category constructors
 */
export const MySqlClient = {
  TypeId: MySqlClientTypeId,
  make,
  layer,
  MySqlClient: MySqlClientService,
}

/**
 * Probe an arbitrary value for the MySqlClient brand.
 *
 * @since 0.1.0
 * @category guards
 */
export const isMySqlClient = (v: unknown): v is MySqlClient => {
  if (v === null) return false
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (v as { readonly [MySqlClientTypeId]?: unknown })[MySqlClientTypeId] === MySqlClientTypeId
}
