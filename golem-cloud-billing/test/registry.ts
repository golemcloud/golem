import { randomUUID } from "node:crypto"
import { readdir, readFile } from "node:fs/promises"
import { Redacted } from "effect"
import pg from "pg"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import { postgres, testConfig } from "./support.ts"

const migrations = new URL("../../golem-registry-service/db/migration/postgres/", import.meta.url)

/** The registry tables that the database role of the billing app can read. */
export const meteringTables = [
  "account_usage_stats",
  "account_monthly_policy_revisions",
  "account_monthly_policy_attribution"
]

/** The registry columns that the database role of the billing app can read, for each table. */
export const accountColumns: Readonly<Record<string, ReadonlyArray<string>>> = {
  accounts: ["account_id", "deleted_at"]
}

export interface Dimensions {
  readonly computeFuel?: bigint
  readonly memoryGbSeconds?: bigint
  readonly memoryRemainder?: bigint
  readonly durableByteSeconds?: bigint
  readonly durableRemainder?: bigint
  readonly ephemeralByteSeconds?: bigint
  readonly ephemeralRemainder?: bigint
}

export interface Period {
  readonly year: number
  readonly month: number
}

/**
 * Ends the pool and waits until each of its clients has closed its connection. `pool.end()`
 * resolves before that. A forced `DROP DATABASE` can stop a backend that has not yet read the
 * Terminate message of its client, and the pool then raises the FATAL error as an unhandled
 * `error` event.
 */
export const closePool = async (pool: pg.Pool): Promise<void> => {
  let open = pool.totalCount
  const closed = new Promise<void>((resolve) => {
    if (open === 0) {
      resolve()
    }
    pool.on("remove", () => {
      open -= 1
      if (open === 0) {
        resolve()
      }
    })
  })
  await pool.end()
  await closed
}

const usageKeyOf = (period: Period) => `${period.year}-${String(period.month).padStart(2, "0")}`

/**
 * A database with the tables of the registry service in `public`, as in the hosted deployment,
 * and a billing app configuration that connects as a role that can read only the metering tables.
 * In a database that other test files share, the DDL of the registry migrations would race with
 * the schema introspection of Better Auth.
 */
export class RegistryDatabase {
  readonly database = `registry_${randomUUID().replaceAll("-", "")}`
  readonly role = `${this.database}_billing`
  readonly config: BillingConfigShape
  readonly admin: pg.Pool
  private readonly server: pg.Pool

  constructor() {
    const db = postgres()
    this.server = new pg.Pool({ ...db, user: db.username, max: 1 })
    this.admin = new pg.Pool({ ...db, user: db.username, database: this.database, max: 2 })
    const base = testConfig()
    this.config = {
      ...base,
      db: {
        ...base.db,
        database: this.database,
        username: this.role,
        password: Redacted.make("metering"),
        registrySchema: "public"
      }
    }
  }

  /** Creates the database, applies every registry migration and grants as the setup wizard does. */
  async create(): Promise<void> {
    await this.server.query(`CREATE DATABASE "${this.database}"`)
    const files = (await readdir(migrations)).filter((file) => file.endsWith(".sql")).sort()
    for (const file of files) {
      await this.admin.query(await readFile(new URL(file, migrations), "utf8"))
    }
    // The grants of ci-scripts/setup-cloud-billing.sh in golem-cloud.
    await this.admin.query(`CREATE ROLE "${this.role}" LOGIN PASSWORD 'metering'`)
    await this.admin.query(`CREATE SCHEMA "${this.config.db.schema}" AUTHORIZATION "${this.role}"`)
    await this.admin.query(`GRANT USAGE ON SCHEMA public TO "${this.role}"`)
    await this.admin.query(
      `GRANT SELECT ON ${meteringTables.map((table) => `public.${table}`).join(", ")} TO "${this.role}"`
    )
    for (const [table, columns] of Object.entries(accountColumns)) {
      await this.admin.query(`GRANT SELECT (${columns.join(", ")}) ON public.${table} TO "${this.role}"`)
    }
  }

  /** Drops the database. Every other pool that connects to it must be closed with `closePool` first. */
  async drop(): Promise<void> {
    await closePool(this.admin)
    await this.server.query(`DROP DATABASE IF EXISTS "${this.database}" WITH (FORCE)`)
    await this.server.query(`DROP ROLE IF EXISTS "${this.role}"`)
    await this.server.end()
  }

  async account(): Promise<string> {
    const id = randomUUID()
    await this.admin.query(
      `INSERT INTO accounts (account_id, email, created_at, updated_at, modified_by, current_revision_id)
       VALUES ($1, $2, now(), now(), $1, 0)`,
      [id, `${id}@example.com`]
    )
    return id
  }

  /** Deletes the account as the registry does: it sets `deleted_at` and keeps the row. */
  async deleteAccount(accountId: string): Promise<void> {
    await this.admin.query("UPDATE accounts SET deleted_at = now(), updated_at = now() WHERE account_id = $1", [
      accountId
    ])
  }

  async revision(
    accountId: string,
    revision: number,
    mode: "hard_limit" | "allow_overage",
    options: { readonly baseline?: Dimensions; readonly resolved?: Dimensions; readonly period: Period }
  ): Promise<void> {
    const baseline = options.baseline ?? {}
    const resolved = options.resolved ?? {}
    await this.admin.query(
      `INSERT INTO account_monthly_policy_revisions (
         account_id, revision, period_year, period_month, mode,
         baseline_compute_fuel, baseline_memory_gb_seconds, baseline_memory_byte_nanoseconds_remainder,
         baseline_durable_storage_byte_seconds, baseline_durable_storage_byte_nanoseconds_remainder,
         baseline_ephemeral_storage_byte_seconds, baseline_ephemeral_storage_byte_nanoseconds_remainder,
         resolved_compute_fuel, resolved_memory_gb_seconds, resolved_durable_storage_byte_seconds,
         resolved_ephemeral_storage_byte_seconds, created_at)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, now())`,
      [
        accountId,
        revision,
        options.period.year,
        options.period.month,
        mode,
        baseline.computeFuel ?? 0n,
        baseline.memoryGbSeconds ?? 0n,
        baseline.memoryRemainder ?? 0n,
        baseline.durableByteSeconds ?? 0n,
        baseline.durableRemainder ?? 0n,
        baseline.ephemeralByteSeconds ?? 0n,
        baseline.ephemeralRemainder ?? 0n,
        resolved.computeFuel ?? 0n,
        resolved.memoryGbSeconds ?? 0n,
        resolved.durableByteSeconds ?? 0n,
        resolved.ephemeralByteSeconds ?? 0n
      ]
    )
  }

  /** One usage update of the registry: the usage that `revision` received in `period`. */
  async attribution(
    accountId: string,
    revision: number,
    usage: Dimensions,
    options: {
      readonly period: Period
      readonly metering?: Partial<Record<"compute" | "memory" | "filesystem", boolean>>
    }
  ): Promise<void> {
    const metering = { compute: true, memory: true, filesystem: true, ...options.metering }
    await this.admin.query(
      `INSERT INTO account_monthly_policy_attribution (
         usage_update_id, account_id, revision, usage_key,
         compute_fuel_delta, memory_gb_seconds_delta, durable_storage_byte_seconds_delta,
         ephemeral_storage_byte_seconds_delta, memory_byte_nanoseconds_remainder,
         durable_storage_byte_nanoseconds_remainder, ephemeral_storage_byte_nanoseconds_remainder,
         compute_metering_enabled, memory_metering_enabled, filesystem_metering_enabled, recorded_at)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, now())`,
      [
        randomUUID(),
        accountId,
        revision,
        usageKeyOf(options.period),
        usage.computeFuel ?? 0n,
        usage.memoryGbSeconds ?? 0n,
        usage.durableByteSeconds ?? 0n,
        usage.ephemeralByteSeconds ?? 0n,
        usage.memoryRemainder ?? 0n,
        usage.durableRemainder ?? 0n,
        usage.ephemeralRemainder ?? 0n,
        metering.compute,
        metering.memory,
        metering.filesystem
      ]
    )
  }
}
