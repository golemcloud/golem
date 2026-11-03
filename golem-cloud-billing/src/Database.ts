import { Context, Effect, Layer, Redacted } from "effect"
import pg from "pg"
import { BillingConfig, type DbTls, type PostgresConfig } from "./BillingConfig.ts"

export interface DatabaseShape {
  readonly pool: pg.Pool
  /** The PostgreSQL schema that holds every table of the billing app. */
  readonly schema: string
}

export class Database extends Context.Service<Database, DatabaseShape>()(
  "golem-cloud-billing/Database"
) {}

const sslOptions = (tls: DbTls): pg.PoolConfig["ssl"] => {
  switch (tls) {
    case "disable":
      return false
    case "require":
      return { rejectUnauthorized: false }
    case "verify-full":
      return true
  }
}

export const makePool = (config: PostgresConfig): pg.Pool =>
  new pg.Pool({
    host: config.host,
    port: config.port,
    database: config.database,
    user: config.username,
    password: Redacted.value(config.password),
    max: config.maxConnections,
    ssl: sslOptions(config.tls),
    application_name: "golem-cloud-billing",
    connectionTimeoutMillis: 10_000
  })

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
 */
export const withAdvisoryLock = <A>(
  pool: pg.Pool,
  key: string,
  body: () => Promise<A>
): Promise<A> =>
  pool.connect().then(async (client) => {
    try {
      await client.query("SELECT pg_advisory_lock(hashtextextended($1, 0))", [key])
      try {
        return await body()
      } finally {
        await client.query("SELECT pg_advisory_unlock(hashtextextended($1, 0))", [key])
      }
    } finally {
      client.release()
    }
  })
