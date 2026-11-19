import type { BetterAuthOptions } from "better-auth"
import { getMigrations } from "better-auth/db/migration"
import { Data, Effect } from "effect"
import * as Auth from "./Auth.ts"
import { Database, withAdvisoryLock } from "./Database.ts"
import * as Subscriptions from "./Subscriptions.ts"

/**
 * The database migrations of the billing app: the only DDL of the app.
 *
 * `node main.mjs migrate` runs them as the owner role of the schema, before the other workloads
 * start. The web app, the usage push and the cleanup connect as roles that cannot create or change
 * tables, and only check that the tables exist (`Database.requireTables`).
 */

export class MigrationError extends Data.TaggedError("MigrationError")<{
  readonly cause: unknown
}> {
  override get message(): string {
    return `Cannot apply the database migrations: ${Auth.describeCause(this.cause)}`
  }
}

/** Every table of the app: the tables of Better Auth and `Subscriptions.tables`. */
export const appTables: ReadonlyArray<string> = [...Auth.tableNames, ...Subscriptions.tableNames]

/**
 * PostgreSQL error codes of a table, column or schema that another session dropped or changed
 * during the introspection of `getMigrations`. Better Auth reads the columns of every table in
 * the database, also outside its schema, so migrations of the registry service that run at the
 * same time can fail the read. A later attempt sees the finished change.
 */
const concurrentDdlCodes = new Set(["42P01", "42703", "3F000"])

const getMigrationsRetryingConcurrentDdl = async (options: BetterAuthOptions) => {
  for (let attempt = 1; ; attempt++) {
    try {
      return await getMigrations(options)
    } catch (error) {
      const code = typeof error === "object" && error !== null && "code" in error ? error.code : undefined
      if (attempt === 5 || typeof code !== "string" || !concurrentDdlCodes.has(code)) {
        throw error
      }
      await new Promise((resolve) => setTimeout(resolve, 100 * attempt))
    }
  }
}

/**
 * Applies the Better Auth migrations, turns `user.golem_account_id` into a `uuid` column, and
 * creates `Subscriptions.tables`, under an advisory lock, so that two runs apply them one after
 * the other. A run on a migrated schema changes nothing.
 *
 * Better Auth has no `uuid` field type, so it creates the column as `text`; its migration check
 * accepts `uuid` for a string field, so later runs leave the column alone.
 *
 * An administrator creates the schema for the owner role, which has no CREATE privilege on the
 * database, so the migration fails when the schema is missing.
 */
export const migrate: Effect.Effect<void, MigrationError, Database> = Effect.gen(function* () {
  const { pool, config, schema } = yield* Database
  const services = yield* Effect.context<never>()
  const options = {
    ...Auth.schemaOptions(pool),
    logger: Auth.loggerOptions(Auth.logWith(services))
  } satisfies BetterAuthOptions

  yield* Effect.tryPromise({
    try: () =>
      withAdvisoryLock(config, "golem-cloud-billing/migrations", async () => {
        const existing = await pool.query("SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = $1", [schema])
        if (existing.rowCount === 0) {
          throw new Error(`the schema ${schema} does not exist; an administrator creates it for the owner role of the app`)
        }
        const { runMigrations } = await getMigrationsRetryingConcurrentDdl(options)
        await runMigrations()
        const column = await pool.query<{ data_type: string }>(
          `SELECT data_type FROM information_schema.columns
           WHERE table_schema = $1 AND table_name = 'user' AND column_name = 'golem_account_id'`,
          [schema]
        )
        if (column.rows[0]?.data_type !== "uuid") {
          await pool.query(
            `ALTER TABLE "${schema}"."user"
             ALTER COLUMN golem_account_id TYPE uuid USING golem_account_id::uuid`
          )
        }
        await pool.query(Subscriptions.tables)
      }),
    catch: (cause) => new MigrationError({ cause })
  })
})
