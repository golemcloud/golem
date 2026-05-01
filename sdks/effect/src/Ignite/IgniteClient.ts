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
import { SqlError, SqlSyntaxError } from "effect/unstable/sql/SqlError"
import * as Statement from "effect/unstable/sql/Statement"
import {
  type DbConnection,
  type DbResultStream,
  type DbTransaction,
  type DbValue,
} from "golem:rdbms/ignite2@1.5.0"
import { IgniteHostClient } from "../host/IgniteHostClient.js"
import { READ_PREFIX_RE, sqlErrorFor } from "../internal/rdbmsShared.js"
import { decodeRows, decodeRowsValues, encodeAllParams } from "./internal/codec.js"

export {
  Ignite,
  IgniteParamTag,
  type IgniteParam,
  type IgniteUuid,
  isIgniteParam,
} from "./Ignite.js"

const ATTR_DB_SYSTEM_NAME = "db.system.name"

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * Unique symbol stamped on every {@link IgniteClient} instance so
 * consumers can reliably distinguish an Ignite client. Keyed via
 * `Symbol.for` so multiple module copies still agree on the same key.
 *
 * @since 0.1.0
 * @category symbols
 */
export const IgniteClientTypeId: unique symbol = Symbol.for(
  "effect-golem/IgniteClient",
) as IgniteClientTypeId

/**
 * @since 0.1.0
 * @category symbols
 */
export type IgniteClientTypeId = typeof IgniteClientTypeId

const IgniteConnectionTxSymbol: unique symbol = Symbol.for(
  "effect-golem/IgniteClient/__txContext",
) as typeof IgniteConnectionTxSymbol

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * How temporal values (db-date / db-timestamp) are decoded from rows.
 *
 * @since 0.1.0
 * @category models
 */
export type TemporalDecodeMode = "raw" | "date"

/**
 * Configuration accepted by {@link IgniteClient.make} / {@link IgniteClient.layer}.
 *
 * @since 0.1.0
 * @category models
 */
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
 *
 * @since 0.1.0
 * @category models
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
 *
 * @since 0.1.0
 * @category host services
 */
export class IgniteClientService extends Context.Service<IgniteClientService, IgniteClient>()(
  "effect-golem/IgniteClient",
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
): Effect.Effect<IgniteClient, SqlError, Scope.Scope | Reactivity.Reactivity | IgniteHostClient> =>
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

    const host = yield* IgniteHostClient
    const db = yield* Effect.try({
      try: () => host.open(config.connectionAddress),
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

const make = (
  config: IgniteClientConfig,
): Effect.Effect<IgniteClient, SqlError, Scope.Scope | IgniteHostClient> =>
  Effect.provide(makeImpl(config), Reactivity.layer)

const layer = (
  config: IgniteClientConfig,
): Layer.Layer<IgniteClientService | Client.SqlClient, SqlError, IgniteHostClient> =>
  Layer.effectContext(
    Effect.map(makeImpl(config), (client) =>
      Context.make(IgniteClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide(Reactivity.layer))

/**
 * Public namespace mirror used by `import { IgniteClient } from "effect-golem/ignite2"`.
 *
 * @since 0.1.0
 * @category constructors
 */
export const IgniteClient = {
  TypeId: IgniteClientTypeId,
  make,
  layer,
  IgniteClient: IgniteClientService,
}

/**
 * Probe an arbitrary value for the IgniteClient brand.
 *
 * @since 0.1.0
 * @category guards
 */
export const isIgniteClient = (v: unknown): v is IgniteClient => {
  if (v === null) return false
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (
    (v as { readonly [IgniteClientTypeId]?: unknown })[IgniteClientTypeId] === IgniteClientTypeId
  )
}
