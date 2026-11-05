import { Effect, Layer, Redacted } from "effect"
import { FetchHttpClient } from "effect/http"
import type pg from "pg"
import { afterAll, beforeAll, describe, expect, it } from "vitest"
import * as App from "../src/App.ts"
import { BillingConfig, type BillingConfigShape } from "../src/BillingConfig.ts"
import { makePool } from "../src/Database.ts"
import { dropSchema, queryPool, testConfig } from "./support.ts"

/** Builds the services once, which applies the migrations, then releases them. */
const startServices = (config: BillingConfigShape) =>
  Effect.runPromise(
    Effect.scoped(
      Layer.build(
        App.services.pipe(
          Layer.provide(FetchHttpClient.layer),
          Layer.provide(Layer.succeed(BillingConfig, config))
        )
      )
    )
  )

/** The tables of Better Auth and of the Polar subscriptions and orders, in the order of their names. */
const appTables = [
  "account",
  "polar_orders",
  "polar_subscriptions",
  "polar_webhook_deliveries",
  "session",
  "user",
  "verification"
]

describe("migrations", () => {
  let config: BillingConfigShape
  let pool: pg.Pool

  beforeAll(() => {
    config = testConfig()
    pool = queryPool(config)
  })

  afterAll(async () => {
    await dropSchema(pool, config.db.schema)
    await pool.end()
  })

  const columns = async (table: string) =>
    Object.fromEntries(
      (
        await pool.query<{ column_name: string; data_type: string }>(
          `SELECT column_name, data_type FROM information_schema.columns
           WHERE table_schema = $1 AND table_name = $2`,
          [config.db.schema, table]
        )
      ).rows.map((row) => [row.column_name, row.data_type])
    )

  it("creates the tables of the billing app in its schema only", async () => {
    await startServices(config)

    const tablesIn = async (schema: string) =>
      (
        await pool.query<{ table_name: string }>(
          `SELECT table_name FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name`,
          [schema]
        )
      ).rows.map((row) => row.table_name)

    expect(await tablesIn(config.db.schema)).toEqual(appTables)
    // The registry service keeps its tables in `public`.
    expect(await tablesIn("public")).toEqual([])
  })

  it("stores the Golem account ID as a nullable unique uuid", async () => {
    await startServices(config)

    expect((await columns("user"))["golem_account_id"]).toBe("uuid")
    expect((await columns("session"))["golem_token"]).toBe("text")
    const unique = await pool.query(
      `SELECT 1 FROM pg_indexes WHERE schemaname = $1 AND tablename = 'user'
       AND indexdef LIKE 'CREATE UNIQUE INDEX%(golem_account_id)'`,
      [config.db.schema]
    )
    expect(unique.rowCount).toBe(1)
  })

  it("starts again on a migrated schema", async () => {
    await startServices(config)
    await expect(startServices(config)).resolves.toBeDefined()
  })

  it("starts with a pool of one connection", async () => {
    const single = testConfig()
    try {
      await expect(startServices({ ...single, db: { ...single.db, maxConnections: 1 } })).resolves.toBeDefined()
    } finally {
      await dropSchema(pool, single.db.schema)
    }
  })

  it("lets replicas that start together migrate one after the other", async () => {
    const concurrent = testConfig()
    try {
      await Promise.all([startServices(concurrent), startServices(concurrent), startServices(concurrent)])
      expect((await pool.query(`SELECT count(*)::int AS n FROM "${concurrent.db.schema}"."user"`)).rows).toEqual([
        { n: 0 }
      ])
    } finally {
      await dropSchema(pool, concurrent.db.schema)
    }
  })

  it("runs as a role that owns only its schema and cannot read the registry tables", async () => {
    // The setup of the hosted deployment: the role owns the schema of the app and has no
    // privilege on the database or on the tables of the registry service in `public`.
    const restricted = testConfig()
    // A role name that differs from the schema name: the default `search_path` starts with
    // `"$user"`, which would otherwise select the schema of the app by name.
    const role = `${restricted.db.schema}_role`
    const registryTable = `"public"."${restricted.db.schema}_tokens"`
    await pool.query(`CREATE ROLE "${role}" LOGIN PASSWORD 'restricted'`)
    await pool.query(`CREATE SCHEMA "${restricted.db.schema}" AUTHORIZATION "${role}"`)
    await pool.query(`CREATE TABLE ${registryTable} (secret text NOT NULL)`)
    const rolePool = makePool({
      ...restricted.db,
      username: role,
      password: Redacted.make("restricted"),
      maxConnections: 1
    })
    try {
      await startServices({
        ...restricted,
        db: { ...restricted.db, username: role, password: Redacted.make("restricted") }
      })

      const tables = await pool.query<{ table_name: string; table_owner: string }>(
        `SELECT tablename AS table_name, tableowner AS table_owner FROM pg_tables
         WHERE schemaname = $1 ORDER BY tablename`,
        [restricted.db.schema]
      )
      expect(tables.rows).toEqual(
        appTables.map((table_name) => ({ table_name, table_owner: role }))
      )
      await expect(rolePool.query(`SELECT secret FROM ${registryTable}`)).rejects.toThrow(/permission denied/)
    } finally {
      await rolePool.end()
      await pool.query(`DROP TABLE IF EXISTS ${registryTable}`)
      await dropSchema(pool, restricted.db.schema)
      await pool.query(`DROP ROLE IF EXISTS "${role}"`)
    }
  })
})
