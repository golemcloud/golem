import { randomUUID } from "node:crypto"
import { Effect, Layer, Redacted } from "effect"
import { FetchHttpClient } from "effect/http"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import * as App from "../src/App.ts"
import { BillingConfig } from "../src/BillingConfig.ts"
import * as Cleanup from "../src/Cleanup.ts"
import * as Database from "../src/Database.ts"
import * as Polar from "../src/Polar.ts"
import * as RegistryAccounts from "../src/RegistryAccounts.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import { closePool, RegistryDatabase } from "./registry.ts"
import { queryPool } from "./support.ts"

const polarUrl = "https://polar.test"
const polarToken = "polar-cleanup-token"
const meteredPriceId = "price_compute"

/** Polar's `DELETE /v1/customers/external/{external_id}`. */
class FakePolar {
  readonly deletes: Array<{ readonly externalId: string; readonly authorization: string | null }> = []
  readonly customers = new Set<string>()
  failures = 0

  readonly fetch = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init)
    const match = /^\/v1\/customers\/external\/([^/]+)$/.exec(new URL(request.url).pathname)
    if (request.method !== "DELETE" || !request.url.startsWith(polarUrl) || match === null) {
      return new Response(`No fake for ${request.method} ${request.url}`, { status: 599 })
    }
    const externalId = decodeURIComponent(match[1]!)
    this.deletes.push({ externalId, authorization: request.headers.get("authorization") })
    if (this.failures > 0) {
      this.failures--
      return Response.json({ detail: "unavailable" }, { status: 503 })
    }
    if (!this.customers.delete(externalId)) {
      return Response.json({ detail: "Not found" }, { status: 404 })
    }
    return new Response(null, { status: 204 })
  }
}

describe("cleanup of deleted accounts", () => {
  const registry = new RegistryDatabase()
  let pool: pg.Pool
  let polar: FakePolar

  const cleanup = () =>
    Effect.runPromise(
      Cleanup.cleanupDeletedAccounts.pipe(
        Effect.provide(
          Layer.mergeAll(RegistryAccounts.layer, Subscriptions.commandLayer, Polar.layer).pipe(
            Layer.provideMerge(Database.layer),
            Layer.provide(FetchHttpClient.layer),
            Layer.provide(Layer.succeed(BillingConfig, registry.config)),
            Layer.provide(
              Layer.succeed(Polar.PolarConfig, { apiUrl: new URL(polarUrl), accessToken: Redacted.make(polarToken) })
            )
          )
        )
      )
    )

  /** A Better Auth user of the Golem account, with a session and a GitHub account. */
  const user = async (golemAccountId: string) => {
    const id = randomUUID()
    await pool.query(
      `INSERT INTO "user" (id, name, email, "emailVerified", "createdAt", "updatedAt", golem_account_id)
       VALUES ($1, 'Octo Cat', $2, true, now(), now(), $3)`,
      [id, `${id}@example.com`, golemAccountId]
    )
    await pool.query(
      `INSERT INTO session (id, "expiresAt", token, "createdAt", "updatedAt", "userId")
       VALUES ($1, now() + interval '1 day', $1, now(), now(), $2)`,
      [randomUUID(), id]
    )
    await pool.query(
      `INSERT INTO account (id, "accountId", "providerId", "userId", "createdAt", "updatedAt")
       VALUES ($1, $2, 'github', $2, now(), now())`,
      [randomUUID(), id]
    )
    return id
  }

  const rowsOf = async (userId: string) =>
    (
      await pool.query<{ users: number; sessions: number; accounts: number }>(
        `SELECT (SELECT count(*)::int FROM "user" WHERE id = $1) AS users,
                (SELECT count(*)::int FROM session WHERE "userId" = $1) AS sessions,
                (SELECT count(*)::int FROM account WHERE "userId" = $1) AS accounts`,
        [userId]
      )
    ).rows[0]

  const subscribe = async (accountId: string, status = "canceled") => {
    await pool.query(
      `INSERT INTO polar_subscriptions
         (subscription_id, account_id, product_id, status, current_period_end, cancel_at_period_end, modified_at,
          metered_price_ids)
       VALUES ($1, $2, 'paid-product', $3, now(), false, now(), $4)`,
      [`sub_${accountId}`, accountId, status, [meteredPriceId]]
    )
  }

  /** A deleted account with a user and a Polar customer, as after a sign-up and a checkout. */
  const deletedAccount = async () => {
    const accountId = await registry.account()
    const userId = await user(accountId)
    await subscribe(accountId)
    polar.customers.add(accountId)
    await registry.deleteAccount(accountId)
    return { accountId, userId }
  }

  beforeAll(async () => {
    await registry.create()
    pool = queryPool(registry.config)
    vi.stubGlobal("fetch", (input: string | URL | Request, init?: RequestInit) => polar.fetch(input, init))
    // The web app creates the tables of Better Auth.
    await Effect.runPromise(
      Effect.scoped(
        Layer.build(
          App.services.pipe(
            Layer.provide(FetchHttpClient.layer),
            Layer.provide(Layer.succeed(BillingConfig, registry.config))
          )
        )
      )
    )
  }, 60_000)

  afterAll(async () => {
    vi.unstubAllGlobals()
    await closePool(pool)
    await registry.drop()
  })

  beforeEach(() => {
    polar = new FakePolar()
  })

  afterEach(async () => {
    await pool.query(
      `TRUNCATE "user", polar_subscriptions, polar_orders, usage_events, deleted_account_cleanups, deletion_holds CASCADE`
    )
  })

  it("deletes the Polar customer and the user of a deleted account once, and keeps its subscriptions", async () => {
    const deleted = await deletedAccount()
    const live = await registry.account()
    const liveUser = await user(live)
    await subscribe(live, "active")
    polar.customers.add(live)

    expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, deletedUsers: 1 })

    expect(polar.deletes).toEqual([{ externalId: deleted.accountId, authorization: `Bearer ${polarToken}` }])
    expect(await rowsOf(deleted.userId)).toEqual({ users: 0, sessions: 0, accounts: 0 })
    expect(await rowsOf(liveUser)).toEqual({ users: 1, sessions: 1, accounts: 1 })
    const kept = await pool.query("SELECT account_id::text AS account_id FROM polar_subscriptions ORDER BY account_id")
    expect(kept.rows.map((row) => row.account_id).sort()).toEqual([deleted.accountId, live].sort())

    expect(await cleanup()).toEqual({ deletedCustomers: 0, unsettled: 0, deletedUsers: 0 })
    expect(polar.deletes).toHaveLength(1)
  })

  it("keeps the customer and the user of a deleted account that owes paid overage", async () => {
    const unpaid = await deletedAccount()
    await pool.query(
      `INSERT INTO polar_orders (order_id, account_id, subscription_id, status, created_at, modified_at, items)
       VALUES ('order_1', $1, $2, 'pending', now(), now(), $3)`,
      [unpaid.accountId, `sub_${unpaid.accountId}`, JSON.stringify([{ productPriceId: meteredPriceId, amount: 345 }])]
    )
    const unbilled = await deletedAccount()
    await pool.query(
      `INSERT INTO usage_events (event_id, account_id, period, dimension, from_micro_units, to_micro_units, sent_at)
       VALUES ('event_1', $1, '2026-11', 'compute', 0, 5, now())`,
      [unbilled.accountId]
    )

    expect(await cleanup()).toEqual({ deletedCustomers: 0, unsettled: 2, deletedUsers: 0 })
    expect(polar.deletes).toEqual([])
    expect(await rowsOf(unpaid.userId)).toEqual({ users: 1, sessions: 1, accounts: 1 })

    await pool.query("UPDATE polar_orders SET status = 'paid'")
    await pool.query("UPDATE usage_events SET sent_at = NULL, held_at = now()")
    expect(await cleanup()).toEqual({ deletedCustomers: 2, unsettled: 0, deletedUsers: 2 })
  })

  it("keeps the customer of a deleted account whose subscription runs to the end of the period", async () => {
    // An administrator removed the deletion hold and deleted the account before the period ended.
    const accountId = await registry.account()
    const userId = await user(accountId)
    await subscribe(accountId, "active")
    await pool.query("UPDATE polar_subscriptions SET cancel_at_period_end = true WHERE account_id = $1", [accountId])
    polar.customers.add(accountId)
    await pool.query("INSERT INTO deletion_holds (account_id, reason) VALUES ($1, 'held')", [accountId])
    await registry.deleteAccount(accountId)
    const recordedHolds = async () =>
      (await pool.query("SELECT reason FROM deletion_holds WHERE account_id = $1", [accountId])).rows

    expect(await cleanup()).toEqual({ deletedCustomers: 0, unsettled: 1, deletedUsers: 0 })
    expect(polar.deletes).toEqual([])
    expect(await rowsOf(userId)).toEqual({ users: 1, sessions: 1, accounts: 1 })
    expect(await recordedHolds()).toEqual([{ reason: "held" }])

    await pool.query("UPDATE polar_subscriptions SET status = 'canceled' WHERE account_id = $1", [accountId])
    expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, deletedUsers: 1 })
    expect(await recordedHolds()).toEqual([])
  })

  it("counts a customer that Polar does not have as deleted", async () => {
    const accountId = await registry.account()
    const userId = await user(accountId)
    await registry.deleteAccount(accountId)

    expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, deletedUsers: 1 })
    expect(polar.deletes.map((request) => request.externalId)).toEqual([accountId])
    expect(await rowsOf(userId)).toEqual({ users: 0, sessions: 0, accounts: 0 })
  })

  it("fails when Polar fails, keeps the user, and deletes both at the next run", async () => {
    const deleted = await deletedAccount()
    polar.failures = 1

    await expect(cleanup()).rejects.toThrow(/1 Polar customers of deleted accounts are not deleted/)
    expect(await rowsOf(deleted.userId)).toEqual({ users: 1, sessions: 1, accounts: 1 })

    expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, deletedUsers: 1 })
    expect(polar.customers.has(deleted.accountId)).toBe(false)
  })
})
