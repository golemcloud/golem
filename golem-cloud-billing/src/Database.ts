import { type Config, Context, Data, Effect, Layer, Redacted, type Scope } from "effect"
import pg from "pg"
import { BillingConfig, type DbTls, type PostgresConfig, postgresConfig } from "./BillingConfig.ts"

export interface DatabaseShape {
  /** A pool whose connections have the schema of the billing app as their `search_path`. */
  readonly pool: pg.Pool
  /** The PostgreSQL schema that holds every table of the billing app. */
  readonly schema: string
  /** The connection settings of the pool, also for connections outside it. */
  readonly config: PostgresConfig
}

export class Database extends Context.Service<Database, DatabaseShape>()(
  "golem-cloud-billing/Database"
) {}

const sslOptions = (tls: DbTls): pg.ClientConfig["ssl"] => {
  switch (tls) {
    case "disable":
      return false
    case "require":
      return { rejectUnauthorized: false }
    case "verify-full":
      return true
  }
}

/** The connection settings of every PostgreSQL connection of the billing app. */
export const connectionOptions = (config: PostgresConfig): pg.ClientConfig => ({
  host: config.host,
  port: config.port,
  database: config.database,
  user: config.username,
  password: Redacted.value(config.password),
  ssl: sslOptions(config.tls),
  application_name: "golem-cloud-billing",
  connectionTimeoutMillis: 10_000,
  // Every unqualified table name resolves in the schema of the billing app. The schema name
  // matches the pattern that BillingConfig checks, so it needs no quoting.
  options: `-c search_path=${config.schema}`
})

export const makePool = (config: PostgresConfig): pg.Pool =>
  new pg.Pool({ ...connectionOptions(config), max: config.maxConnections })

const make = (db: PostgresConfig): Effect.Effect<DatabaseShape, never, Scope.Scope> =>
  Effect.gen(function* () {
    const pool = yield* Effect.acquireRelease(
      Effect.sync(() => makePool(db)),
      (pool) => Effect.promise(() => pool.end())
    )
    const services = yield* Effect.context<never>()
    // An idle client that loses its connection emits `error` on the pool; without a listener
    // Node treats it as an unhandled error and exits.
    pool.on("error", (error) => {
      Effect.runForkWith(services)(
        Effect.logWarning("Idle PostgreSQL connection failed").pipe(
          Effect.annotateLogs({ error: error.message })
        )
      )
    })
    return { pool, schema: db.schema, config: db }
  })

/** The database of the web app, configured by `BillingConfig`. */
export const layer: Layer.Layer<Database, never, BillingConfig> = Layer.effect(
  Database,
  Effect.gen(function* () {
    const { db } = yield* BillingConfig
    return yield* make(db)
  })
)

/**
 * The database configured by the `GOLEM__DB__CONFIG__*` variables alone, for the commands that need
 * no other setting of the web app.
 */
export const layerFromEnv: Layer.Layer<Database, Config.ConfigError> = Layer.effect(
  Database,
  Effect.gen(function* () {
    return yield* make(yield* postgresConfig)
  })
)

/**
 * Runs `body` while holding a session-level advisory lock, so that replicas that start at the
 * same time apply migrations one after the other.
 *
 * The lock has its own connection, outside the pool: `body` uses the pool, which may have a
 * single connection.
 */
export const withAdvisoryLock = async <A>(
  config: PostgresConfig,
  key: string,
  body: () => Promise<A>
): Promise<A> => {
  const client = new pg.Client(connectionOptions(config))
  await client.connect()
  try {
    await client.query("SELECT pg_advisory_lock(hashtextextended($1, 0))", [key])
    try {
      return await body()
    } finally {
      await client.query("SELECT pg_advisory_unlock(hashtextextended($1, 0))", [key])
    }
  } finally {
    await client.end()
  }
}

/** Tables of the app that are missing, or a failed read of the catalog. */
export class SchemaError extends Data.TaggedError("SchemaError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `The schema of the billing app is not ready: ${this.reason}`
  }
}

/**
 * Fails unless every table of `tables` exists in the schema of the app. The web app, the usage
 * push and the cleanup create no table: their database roles have no CREATE privilege on the
 * schema, and PostgreSQL checks it before `IF NOT EXISTS`. The owner role of the schema creates
 * the tables with `node main.mjs migrate` before they start. The catalog shows every table to
 * every role, so the check needs no privilege on the tables.
 */
export const requireTables = (tables: ReadonlyArray<string>): Effect.Effect<void, SchemaError, Database> =>
  Effect.gen(function* () {
    const { pool, schema } = yield* Database
    const result = yield* Effect.tryPromise({
      try: () =>
        pool.query<{ table_name: string }>(
          `SELECT t AS table_name FROM unnest($2::text[]) AS t
            WHERE NOT EXISTS (SELECT 1 FROM pg_catalog.pg_tables WHERE schemaname = $1 AND tablename = t)
            ORDER BY t`,
          [schema, tables]
        ),
      catch: (error) => new SchemaError({ reason: error instanceof Error ? error.message : String(error) })
    })
    const missing = result.rows.map((row) => row.table_name)
    if (missing.length > 0) {
      return yield* new SchemaError({
        reason: `the schema ${schema} has no table ${missing.join(", ")}. Run \`node main.mjs migrate\` as the owner of the schema first`
      })
    }
  })
