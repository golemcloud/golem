import { Context, Effect, Layer, Redacted } from "effect"
import pg from "pg"
import { BillingConfig, type DbTls, type PostgresConfig } from "./BillingConfig.ts"

export interface DatabaseShape {
  /** A pool whose connections have the schema of the billing app as their `search_path`. */
  readonly pool: pg.Pool
  /** The PostgreSQL schema that holds every table of the billing app. */
  readonly schema: string
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

export const layer: Layer.Layer<Database, never, BillingConfig> = Layer.effect(
  Database,
  Effect.gen(function* () {
    const { db } = yield* BillingConfig
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
    return { pool, schema: db.schema }
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
