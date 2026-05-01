/**
 * SQLite adapter for `effect-golem` agents — implements the official
 * `effect/unstable/sql/SqlClient` interface on top of Node's built-in
 * `node:sqlite` (`DatabaseSync`/`StatementSync`) so it works both on
 * Node 22+ (for unit tests and local development) and inside Golem's
 * `wasm-rquickjs` runtime.
 *
 * Why not `@effect/sql-sqlite-node`? That package depends on
 * `better-sqlite3` (a native N-API addon) which cannot run inside
 * `wasm-rquickjs`. This module mirrors the structure of
 * `@effect/sql-sqlite-node`'s adapter (so users can use `SqlSchema`,
 * `SqlResolver`, `Migrator` against it) but delegates to `node:sqlite`
 * primitives and the wasm-rquickjs host extensions for
 * `serializeDatabaseSync` / `restoreDatabaseSync` /
 * `isAutocommitDatabaseSync`.
 *
 * Skipped on purpose:
 * - `executeStream` (no native cursor on `StatementSync`; the
 *   connection returns `Stream.die("not implemented")` to match how
 *   `@effect/sql-sqlite-node` handles the same gap).
 * - `loadExtension` and custom user functions / aggregates (not safely
 *   exposed by `node:sqlite`).
 *
 * @since 1.5.0
 */

import {
  Cache,
  Context,
  type Duration,
  Effect,
  Fiber,
  Layer,
  Scope,
  Semaphore,
  Stream,
} from "effect"
import * as Reactivity from "effect/unstable/reactivity/Reactivity"
import * as Client from "effect/unstable/sql/SqlClient"
import type { Connection } from "effect/unstable/sql/SqlConnection"
import { classifySqliteError, SqlError } from "effect/unstable/sql/SqlError"
import * as Statement from "effect/unstable/sql/Statement"
import type { DatabaseSync, SQLInputValue } from "node:sqlite"
import { serializeDatabaseSync } from "node:sqlite"
import { NodeSqliteClient, NodeSqliteLive } from "../host/NodeSqliteClient.js"

const ATTR_DB_SYSTEM_NAME = "db.system.name"

const sqlError = (cause: unknown, message: string, operation: string): SqlError =>
  new SqlError({ reason: classifySqliteError(cause, { message, operation }) })

// ---------------------------------------------------------------------------
// TypeId / cross-bundle identity
// ---------------------------------------------------------------------------

/**
 * Unique symbol stamped on every {@link SqliteClient} instance so
 * `attachDatabase` (and other consumers) can reliably distinguish a
 * client from a raw `DatabaseSync`. The symbol is keyed via
 * `Symbol.for(...)` so multiple module copies (e.g. one bundled into
 * `effect-golem`'s main bundle for `src/snapshot.ts`'s relative
 * import of `./Sqlite.js`, and one in the standalone
 * `effect-golem/sqlite` sub-import) still agree on the same key.
 *
 * @since 1.5.0
 * @category symbols
 */
export const SqliteClientTypeId: unique symbol = Symbol.for(
  "effect-golem/SqliteClient",
) as SqliteClientTypeId

/**
 * @since 1.5.0
 * @category symbols
 */
export type SqliteClientTypeId = typeof SqliteClientTypeId

/**
 * Globally-keyed symbol used to stash the underlying `DatabaseSync`
 * directly on the SqliteClient object so {@link __getUnderlyingDatabase}
 * works regardless of which bundle the client originated in.
 */
const UnderlyingDbSymbol: unique symbol = Symbol.for(
  "effect-golem/SqliteClient/__db",
) as typeof UnderlyingDbSymbol

const DEFAULT_CACHE_SIZE = 128
const DEFAULT_CACHE_TTL: Duration.Input = "1 hour"

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * The public SqliteClient. Extends the official
 * `effect/unstable/sql/SqlClient` so users can write
 * `yield* sql\`SELECT ...\`` queries, compose with `SqlSchema` /
 * `SqlResolver` / `Migrator`, and resolve the canonical
 * `Client.SqlClient` tag.
 *
 * Augmented with three node:sqlite-specific helpers:
 * - {@link export} — serialize the in-memory DB to raw SQLite bytes
 *   via the `wasm-rquickjs` `serializeDatabaseSync` extension. Used by
 *   the snapshot dispatcher when this client is `attachDatabase`d.
 * - {@link exec} — run one or more parameter-less SQL statements
 *   (DDL or batched seed inserts). Wraps `db.exec(...)`.
 * - {@link config} — the configuration this client was built with.
 *
 * @since 1.5.0
 * @category models
 */
export interface SqliteClient extends Client.SqlClient {
  readonly [SqliteClientTypeId]: SqliteClientTypeId
  readonly config: SqliteClientConfig
  readonly export: Effect.Effect<Uint8Array, SqlError>
  readonly exec: (sql: string) => Effect.Effect<void, SqlError>
  /** Not supported by `node:sqlite`. */
  readonly updateValues: never
}

/**
 * Options accepted by {@link SqliteClient.make} / {@link SqliteClient.layer}.
 *
 * @since 1.5.0
 * @category models
 */
export interface SqliteClientConfig {
  /** SQLite filename — `":memory:"` or a host filesystem path. */
  readonly filename: string
  readonly readonly?: boolean | undefined
  readonly transformResultNames?: ((str: string) => string) | undefined
  readonly transformQueryNames?: ((str: string) => string) | undefined
  /** Maximum number of cached `StatementSync` instances. Default 128. */
  readonly prepareCacheSize?: number | undefined
  /** TTL for cached `StatementSync` instances. Default 1 hour. */
  readonly prepareCacheTTL?: Duration.Input | undefined
  /** Extra `db.system.name` etc. attributes for tracing spans. */
  readonly spanAttributes?: Record<string, unknown> | undefined
}

/**
 * Options accepted by {@link SqliteClient.fromDatabase}.
 *
 * @since 1.5.0
 * @category models
 */
export interface FromDatabaseOptions {
  readonly transformResultNames?: ((str: string) => string) | undefined
  readonly transformQueryNames?: ((str: string) => string) | undefined
  readonly prepareCacheSize?: number | undefined
  readonly prepareCacheTTL?: Duration.Input | undefined
  readonly spanAttributes?: Record<string, unknown> | undefined
  /**
   * If true, register a `Scope` finalizer that calls `db.close()`.
   * Default: false (don't close handles we didn't open).
   */
  readonly closeOnScopeClose?: boolean | undefined
}

// ---------------------------------------------------------------------------
// Effect Context tag
// ---------------------------------------------------------------------------

/**
 * Context tag for resolving an {@link SqliteClient} from the
 * environment. Both this tag and the upstream `Client.SqlClient` tag
 * are populated by {@link SqliteClient.layer}.
 *
 * @since 1.5.0
 * @category host services
 */
export class SqliteClientService extends Context.Service<SqliteClientService, SqliteClient>()(
  "effect-golem/SqliteClient",
) {}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

interface SqliteConnection extends Connection {
  readonly export: Effect.Effect<Uint8Array, SqlError>
  readonly exec: (sql: string) => Effect.Effect<void, SqlError>
}

/**
 * Cheap heuristic for routing a prepared statement to `.all()`
 * (returns rows) vs. `.run()` (returns `{ changes, lastInsertRowid }`).
 * `node:sqlite`'s `StatementSync` does not expose better-sqlite3's
 * `.reader` flag, so we look at the SQL prefix instead. Anything that
 * starts with a select-like keyword goes through `.all()`; everything
 * else goes through `.run()`.
 */
const READ_PREFIX_RE = /^\s*(?:SELECT|WITH|PRAGMA|EXPLAIN|VALUES)\b/i
const isReader = (sql: string): boolean => READ_PREFIX_RE.test(sql)

interface PreparedLike {
  readonly all: (...params: Array<SQLInputValue>) => unknown
  readonly run: (...params: Array<SQLInputValue>) => unknown
}

const makeImpl = (
  config: SqliteClientConfig,
  underlying: { readonly db: DatabaseSync; readonly closeOnFinalize: boolean } | undefined,
): Effect.Effect<SqliteClient, SqlError, Scope.Scope | Reactivity.Reactivity | NodeSqliteClient> =>
  Effect.gen(function* () {
    const compiler = Statement.makeCompilerSqlite(config.transformQueryNames)
    const transformRows = config.transformResultNames
      ? Statement.defaultTransforms(config.transformResultNames).array
      : undefined

    let db: DatabaseSync
    if (underlying !== undefined) {
      db = underlying.db
      if (underlying.closeOnFinalize) {
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            try {
              db.close()
            } catch {
              /* idempotent — close() throws if already closed */
            }
          }),
        )
      }
    } else {
      // NodeSqliteClient.open already registers a Scope finalizer that
      // calls `db.close()` (idempotent), so no additional finalizer is
      // needed on this branch. The host service abstraction lets tests
      // substitute a fake `DatabaseSync` factory via Layer DI.
      const nodeSqlite = yield* NodeSqliteClient
      db = yield* nodeSqlite
        .open(config.filename, { readOnly: config.readonly ?? false })
        .pipe(
          Effect.mapError((cause) => sqlError(cause, `Failed to open ${config.filename}`, "open")),
        )
    }

    const prepareCache = yield* Cache.make({
      capacity: config.prepareCacheSize ?? DEFAULT_CACHE_SIZE,
      timeToLive: config.prepareCacheTTL ?? DEFAULT_CACHE_TTL,
      lookup: (sql: string) =>
        Effect.try({
          try: () => db.prepare(sql) as unknown as PreparedLike,
          catch: (cause) => sqlError(cause, "Failed to prepare statement", "prepare"),
        }),
    })

    const runPrepared = (
      stmt: PreparedLike,
      sql: string,
      params: ReadonlyArray<unknown>,
      raw: boolean,
    ): Effect.Effect<ReadonlyArray<unknown>, SqlError> =>
      Effect.try({
        try: () => {
          const args = params as Array<SQLInputValue>
          if (isReader(sql)) {
            return stmt.all(...args) as ReadonlyArray<unknown>
          }
          const result = stmt.run(...args) as {
            changes: number
            lastInsertRowid: number | bigint
          }
          return raw ? (result as unknown as ReadonlyArray<unknown>) : []
        },
        catch: (cause) => sqlError(cause, "Failed to execute statement", "execute"),
      })

    const run = (sql: string, params: ReadonlyArray<unknown>, raw = false) =>
      Effect.flatMap(Cache.get(prepareCache, sql), (stmt) => runPrepared(stmt, sql, params, raw))

    const runValues = (sql: string, params: ReadonlyArray<unknown>) =>
      Effect.flatMap(Cache.get(prepareCache, sql), (stmt) =>
        Effect.try({
          try: () => {
            const args = params as Array<SQLInputValue>
            if (isReader(sql)) {
              const rows = stmt.all(...args) as Array<Record<string, unknown>>
              // node:sqlite has no `statement.raw(true)` toggle; emulate
              // by extracting the column values in declaration order.
              return rows.map((row) => Object.values(row)) as ReadonlyArray<ReadonlyArray<unknown>>
            }
            stmt.run(...args)
            return [] as ReadonlyArray<ReadonlyArray<unknown>>
          },
          catch: (cause) => sqlError(cause, "Failed to execute statement", "executeValues"),
        }),
      )

    const connection: SqliteConnection = {
      execute(sql, params, transform) {
        const eff = run(sql, params) as Effect.Effect<ReadonlyArray<object>, SqlError>
        return transform ? Effect.map(eff, transform) : eff
      },
      executeRaw(sql, params) {
        return run(sql, params, true)
      },
      executeValues(sql, params) {
        return runValues(sql, params)
      },
      executeUnprepared(sql, params, transform) {
        const eff = Effect.flatMap(
          Effect.try({
            try: () => db.prepare(sql) as unknown as PreparedLike,
            catch: (cause) => sqlError(cause, "Failed to prepare statement", "prepareUnprepared"),
          }),
          (stmt) => runPrepared(stmt, sql, params ?? [], false),
        ) as Effect.Effect<ReadonlyArray<object>, SqlError>
        return transform ? Effect.map(eff, transform) : eff
      },
      executeStream() {
        return Stream.die("executeStream is not implemented for node:sqlite")
      },
      export: Effect.try({
        try: () => serializeDatabaseSync(db),
        catch: (cause) => sqlError(cause, "Failed to export database", "export"),
      }),
      exec(sqlText) {
        return Effect.try({
          try: () => db.exec(sqlText),
          catch: (cause) => sqlError(cause, "Failed to exec SQL", "exec"),
        })
      },
    }

    const semaphore = yield* Semaphore.make(1)
    const acquirer = semaphore.withPermits(1)(Effect.succeed(connection))
    const transactionAcquirer = Effect.uninterruptibleMask((restore) => {
      const fiber = Fiber.getCurrent()!
      const scope = Context.getUnsafe(fiber.context, Scope.Scope)
      return Effect.as(
        Effect.tap(restore(semaphore.take(1)), () =>
          Scope.addFinalizer(scope, semaphore.release(1)),
        ),
        connection,
      )
    })

    const baseClient = yield* Client.make({
      acquirer,
      compiler,
      transactionAcquirer,
      spanAttributes: [
        ...(config.spanAttributes ? Object.entries(config.spanAttributes) : []),
        [ATTR_DB_SYSTEM_NAME, "sqlite"],
      ],
      transformRows,
    })

    const exportEff: Effect.Effect<Uint8Array, SqlError> = Effect.flatMap(
      acquirer,
      (c) => (c as SqliteConnection).export,
    )
    const execFn = (sqlText: string): Effect.Effect<void, SqlError> =>
      Effect.flatMap(acquirer, (c) => (c as SqliteConnection).exec(sqlText))

    const client = Object.assign(baseClient, {
      [SqliteClientTypeId]: SqliteClientTypeId as SqliteClientTypeId,
      [UnderlyingDbSymbol]: db,
      config,
      export: exportEff,
      exec: execFn,
    }) as unknown as SqliteClient
    return client
  })

// ---------------------------------------------------------------------------
// Public factories
// ---------------------------------------------------------------------------

/**
 * Open a fresh `DatabaseSync` and wrap it in a {@link SqliteClient}.
 * Registers a Scope finalizer that closes the underlying handle when
 * the surrounding scope is released. Reactivity and the
 * {@link NodeSqliteClient} host service are provided automatically so
 * user-facing `make` keeps its compact `Effect<…, SqlError, Scope>`
 * signature.
 */
const make = (config: SqliteClientConfig): Effect.Effect<SqliteClient, SqlError, Scope.Scope> =>
  Effect.provide(makeImpl(config, undefined), [Reactivity.layer, NodeSqliteLive])

/**
 * Wrap an externally-owned `DatabaseSync` in a {@link SqliteClient}.
 * By default the underlying handle is **not** closed when the scope
 * ends — pass `closeOnScopeClose: true` to enable that.
 *
 * The {@link NodeSqliteClient} layer is supplied for type-uniformity
 * with {@link make} even though this branch never yields it (the
 * `underlying` path bypasses the constructor service).
 */
const fromDatabase = (
  db: DatabaseSync,
  options: FromDatabaseOptions = {},
): Effect.Effect<SqliteClient, SqlError, Scope.Scope> =>
  Effect.provide(
    makeImpl(
      {
        filename: ":memory:",
        transformResultNames: options.transformResultNames,
        transformQueryNames: options.transformQueryNames,
        prepareCacheSize: options.prepareCacheSize,
        prepareCacheTTL: options.prepareCacheTTL,
        spanAttributes: options.spanAttributes,
      },
      { db, closeOnFinalize: options.closeOnScopeClose ?? false },
    ),
    [Reactivity.layer, NodeSqliteLive],
  )

/**
 * Like {@link make} but returns a Layer that provides both the
 * {@link SqliteClientService} and the upstream {@link Client.SqlClient}
 * tags. Mirrors `@effect/sql-sqlite-node`'s `layer` behaviour so
 * `SqlSchema` / `SqlResolver` / `Migrator` see the canonical
 * `effect/sql/SqlClient` service.
 */
const layer = (
  config: SqliteClientConfig,
): Layer.Layer<SqliteClientService | Client.SqlClient, SqlError> =>
  Layer.effectContext(
    Effect.map(makeImpl(config, undefined), (client) =>
      Context.make(SqliteClientService, client).pipe(Context.add(Client.SqlClient, client)),
    ),
  ).pipe(Layer.provide([Reactivity.layer, NodeSqliteLive]))

/**
 * Public namespace mirror used by `import { SqliteClient } from "effect-golem/sqlite"`.
 *
 * @since 1.5.0
 * @category constructors
 */
export const SqliteClient = {
  TypeId: SqliteClientTypeId,
  make,
  layer,
  fromDatabase,
  SqliteClient: SqliteClientService,
}

// ---------------------------------------------------------------------------
// Helpers consumed by `src/snapshot.ts`
// ---------------------------------------------------------------------------

/**
 * Probe an arbitrary value for the SqliteClient brand.
 *
 * @since 1.5.0
 * @category guards
 */
export const isSqliteClient = (v: unknown): v is SqliteClient => {
  if (v === null) return false
  // The SqliteClient is callable (it extends `effect/unstable/sql`'s
  // `Constructor`), so it shows up as `typeof === "function"` — accept
  // both function and object targets here.
  const kind = typeof v
  if (kind !== "object" && kind !== "function") return false
  return (
    (v as { readonly [SqliteClientTypeId]?: unknown })[SqliteClientTypeId] === SqliteClientTypeId
  )
}

/**
 * Internal: extract the underlying `DatabaseSync` from a
 * {@link SqliteClient}. Used by `snapshot.ts` to capture the handle
 * for `serializeDatabaseSync` / `restoreDatabaseSync`. NOT part of the
 * public surface.
 *
 * @internal
 * @since 1.5.0
 */
export const __getUnderlyingDatabase = (client: SqliteClient): DatabaseSync => {
  const db = (client as unknown as Record<symbol, unknown>)[UnderlyingDbSymbol] as
    | DatabaseSync
    | undefined
  if (db === undefined) {
    throw new Error("SqliteClient was not built by effect-golem; cannot extract DatabaseSync")
  }
  return db
}

/**
 * Re-export for tests / advanced users.
 *
 * @since 1.5.0
 * @category re-exports
 */
export { isAutocommitDatabaseSync, serializeDatabaseSync } from "node:sqlite"
