import { Effect, Layer, Redacted } from "effect"
import { FetchHttpClient } from "effect/http"
import pg from "pg"
import { afterAll, beforeAll, describe, expect, it } from "vitest"
import * as App from "../src/App.ts"
import { BillingConfig, type BillingConfigShape } from "../src/BillingConfig.ts"
import * as Cleanup from "../src/Cleanup.ts"
import * as Database from "../src/Database.ts"
import * as Metering from "../src/Metering.ts"
import * as Migrations from "../src/Migrations.ts"
import * as Polar from "../src/Polar.ts"
import * as RegistryAccounts from "../src/RegistryAccounts.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import * as UsagePush from "../src/UsagePush.ts"
import { migrate, RegistryDatabase, type Workload } from "./registry.ts"
import { dropSchema, queryPool, testConfig } from "./support.ts"

/**
 * The tables of Better Auth, of the Polar subscriptions and orders, of the usage push, of the
 * cleanup, of the deletion holds, and of the usage settlements and their history, in the order of
 * their names.
 */
const appTables = [
  "account",
  "deleted_account_cleanups",
  "deletion_holds",
  "polar_orders",
  "polar_subscriptions",
  "polar_webhook_deliveries",
  "session",
  "usage_events",
  "usage_settlement_history",
  "usage_settlements",
  "user",
  "verification"
]

const polarConfig = Layer.succeed(Polar.PolarConfig, {
  apiUrl: new URL("https://polar.test"),
  accessToken: Redacted.make("polar-test-token")
})

/** Builds the services of the web app once, then releases them. */
const startWebApp = (config: BillingConfigShape) =>
  Effect.runPromise(
    Effect.scoped(
      Layer.build(
        App.services.pipe(Layer.provide(FetchHttpClient.layer), Layer.provide(Layer.succeed(BillingConfig, config)))
      )
    )
  )

/** One run of `push-usage` with `config`. */
const pushUsage = (config: BillingConfigShape) =>
  Effect.runPromise(
    UsagePush.pushUsage(new Date()).pipe(
      Effect.provide(
        Layer.mergeAll(Metering.layer, Polar.layer).pipe(
          Layer.provideMerge(Database.layer),
          Layer.provide(FetchHttpClient.layer),
          Layer.provide(Layer.succeed(BillingConfig, config)),
          Layer.provide(polarConfig)
        )
      )
    )
  )

/** One run of `cleanup-deleted-accounts` with `config`. */
const cleanup = (config: BillingConfigShape) =>
  Effect.runPromise(
    Cleanup.cleanupDeletedAccounts.pipe(
      Effect.provide(
        Layer.mergeAll(RegistryAccounts.layer, Subscriptions.layer, Polar.layer).pipe(
          Layer.provideMerge(Database.layer),
          Layer.provide(FetchHttpClient.layer),
          Layer.provide(Layer.succeed(BillingConfig, config)),
          Layer.provide(polarConfig)
        )
      )
    )
  )

describe("migrate", () => {
  let pool: pg.Pool
  const schemas: Array<string> = []

  beforeAll(() => {
    pool = queryPool(testConfig())
  })

  afterAll(async () => {
    for (const schema of schemas) {
      await dropSchema(pool, schema)
    }
    await pool.end()
  })

  /** A configuration with a new schema that an administrator created, with no tables. */
  const emptySchema = async (): Promise<BillingConfigShape> => {
    const config = testConfig()
    schemas.push(config.db.schema)
    await pool.query(`CREATE SCHEMA "${config.db.schema}"`)
    return config
  }

  const tablesIn = async (schema: string) =>
    (
      await pool.query<{ table_name: string }>(
        `SELECT table_name FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name`,
        [schema]
      )
    ).rows.map((row) => row.table_name)

  const columnsIn = async (schema: string) =>
    (
      await pool.query<{ table_name: string; column_name: string; data_type: string }>(
        `SELECT table_name, column_name, data_type FROM information_schema.columns
         WHERE table_schema = $1 ORDER BY table_name, column_name`,
        [schema]
      )
    ).rows

  it("creates every table of the billing app in its schema only", async () => {
    const config = await emptySchema()

    await migrate(config)

    expect(await tablesIn(config.db.schema)).toEqual(appTables)
    expect([...Migrations.appTables].sort()).toEqual(appTables)
    // The registry service keeps its tables in `public`.
    expect(await tablesIn("public")).toEqual([])
  })

  it("stores the Golem account ID as a nullable unique uuid", async () => {
    const config = await emptySchema()

    await migrate(config)

    const columns = (await columnsIn(config.db.schema)).filter((column) => column.table_name !== "verification")
    expect(columns).toContainEqual({ table_name: "user", column_name: "golem_account_id", data_type: "uuid" })
    expect(columns).toContainEqual({ table_name: "session", column_name: "golem_token", data_type: "text" })
    const unique = await pool.query(
      `SELECT 1 FROM pg_indexes WHERE schemaname = $1 AND tablename = 'user'
       AND indexdef LIKE 'CREATE UNIQUE INDEX%(golem_account_id)'`,
      [config.db.schema]
    )
    expect(unique.rowCount).toBe(1)
  })

  it("changes nothing on a migrated schema", async () => {
    const config = await emptySchema()
    await migrate(config)
    const before = await columnsIn(config.db.schema)

    await migrate(config)

    expect(await columnsIn(config.db.schema)).toEqual(before)
  })

  it("migrates with a pool of one connection", async () => {
    const config = await emptySchema()

    await expect(migrate({ ...config, db: { ...config.db, maxConnections: 1 } })).resolves.toBeUndefined()
  })

  it("lets runs that start together migrate one after the other", async () => {
    const config = await emptySchema()

    await Promise.all([migrate(config), migrate(config), migrate(config)])

    expect(await tablesIn(config.db.schema)).toEqual(appTables)
  })

  it("fails with a message that names the schema when the schema is missing, and creates nothing", async () => {
    const config = testConfig()

    await expect(migrate(config)).rejects.toThrow(`the schema ${config.db.schema} does not exist`)

    const schema = await pool.query("SELECT 1 FROM pg_namespace WHERE nspname = $1", [config.db.schema])
    expect(schema.rowCount).toBe(0)
  })

  it("lets the web app, the usage push and the cleanup create no table, and name migrate when one is missing", async () => {
    // The test user may create tables, so an empty schema afterwards shows that none of them tried.
    const config = await emptySchema()
    const migrateFirst = /has no table .*\. Run `node main\.mjs migrate` as the owner of the schema first/

    await expect(startWebApp(config)).rejects.toThrow(migrateFirst)
    await expect(pushUsage(config)).rejects.toThrow(migrateFirst)
    await expect(cleanup(config)).rejects.toThrow(migrateFirst)

    expect(await tablesIn(config.db.schema)).toEqual([])
  })
})

describe("database roles", () => {
  const registry = new RegistryDatabase()
  const workloads: ReadonlyArray<Workload> = ["web", "push", "cleanup"]

  beforeAll(() => registry.create(), 60_000)
  afterAll(() => registry.drop())

  /** Runs `body` with a connection as the role of `config`. */
  const asRole = async <A>(config: BillingConfigShape, body: (client: pg.Client) => Promise<A>): Promise<A> => {
    const client = new pg.Client(Database.connectionOptions(config.db))
    await client.connect()
    try {
      return await body(client)
    } finally {
      await client.end()
    }
  }

  it("lets the owner of the schema own every table, with no privilege on the registry tables", async () => {
    const tables = await registry.admin.query<{ table_name: string; table_owner: string }>(
      `SELECT tablename AS table_name, tableowner AS table_owner FROM pg_tables
       WHERE schemaname = $1 ORDER BY tablename`,
      [registry.ownerConfig.db.schema]
    )
    expect(tables.rows).toEqual(appTables.map((table_name) => ({ table_name, table_owner: registry.roles.owner })))

    await asRole(registry.ownerConfig, (client) =>
      expect(client.query("SELECT 1 FROM public.accounts")).rejects.toThrow(/permission denied/)
    )
  })

  it.each(workloads)("does not let the %s role change the schema or write the usage settlements", async (workload) => {
    const accountId = await registry.account()

    await asRole(registry.configs[workload], async (client) => {
      await expect(client.query("CREATE TABLE IF NOT EXISTS usage_events (event_id TEXT)")).rejects.toThrow(
        /permission denied/
      )
      await expect(
        client.query("INSERT INTO usage_settlements (account_id, settled_through) VALUES ($1, now())", [accountId])
      ).rejects.toThrow(/permission denied/)
      await expect(
        client.query(
          `INSERT INTO usage_settlement_history (account_id, settled_through, operator, reason)
           VALUES ($1, now(), 'operator', 'reason')`,
          [accountId]
        )
      ).rejects.toThrow(/permission denied/)
      await expect(client.query("SELECT 1 FROM usage_settlement_history")).rejects.toThrow(/permission denied/)
    })
  })

  it("starts the web app, the usage push and the cleanup with the grants of their roles", async () => {
    await expect(startWebApp(registry.configs.web)).resolves.toBeDefined()
    await expect(pushUsage(registry.configs.push)).resolves.toEqual({ created: 0, sent: 0, held: 0 })
    await expect(cleanup(registry.configs.cleanup)).resolves.toEqual({ deletedCustomers: 0, unsettled: 0, deletedUsers: 0 })
  })
})
