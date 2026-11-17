import { randomUUID } from "node:crypto"
import { Effect, Layer, ManagedRuntime } from "effect"
import { afterAll, beforeAll, describe, expect, it } from "vitest"
import { BillingConfig, type BillingConfigShape } from "../src/BillingConfig.ts"
import * as Database from "../src/Database.ts"
import * as RegistryAccounts from "../src/RegistryAccounts.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import { RegistryDatabase } from "./registry.ts"

describe("account lock", () => {
  const registry = new RegistryDatabase()

  beforeAll(async () => {
    await registry.create()
  }, 60_000)

  afterAll(async () => {
    await registry.drop()
  })

  /** The services of the web app on a pool of `maxConnections` connections. */
  const servicesWith = (maxConnections: number) => {
    const config: BillingConfigShape = {
      ...registry.configs.web,
      db: { ...registry.configs.web.db, maxConnections }
    }
    return ManagedRuntime.make(
      Layer.mergeAll(Subscriptions.layer, RegistryAccounts.layer).pipe(
        Layer.provide(Database.layer),
        Layer.provide(Layer.succeed(BillingConfig, config))
      )
    )
  }

  const subscription = (accountId: string, modifiedAt: string): Subscriptions.Subscription => ({
    subscriptionId: `sub_${accountId}`,
    accountId,
    productId: "paid-product",
    status: "active",
    currentPeriodEnd: new Date("2026-12-04T10:00:00Z"),
    cancelAtPeriodEnd: false,
    endedAt: undefined,
    modifiedAt,
    meteredPriceIds: [],
    pastDueAt: undefined
  })

  it("runs the queries of the lock on its connection, also in a pool of one connection", async () => {
    const services = servicesWith(1)
    try {
      const accountId = await registry.account()
      const result = await services.runPromise(
        Effect.gen(function* () {
          const subscriptions = yield* Subscriptions.Subscriptions
          const registryAccounts = yield* RegistryAccounts.RegistryAccounts
          return yield* subscriptions.withAccountLock(
            accountId,
            Effect.gen(function* () {
              yield* subscriptions.record(subscription(accountId, "2026-11-04T10:00:00Z"))
              return {
                subscriptions: (yield* subscriptions.ofAccount(accountId, undefined)).length,
                deleted: yield* registryAccounts.isDeleted(accountId)
              }
            })
          )
        })
      )

      expect(result).toEqual({ subscriptions: 1, deleted: false })
    } finally {
      await services.dispose()
    }
  })

  it("runs ten deliveries of one account at once on a pool of two connections", async () => {
    // The holder keeps the lock for a moment, while the other deliveries wait in the queue of the
    // process.
    const services = servicesWith(2)
    try {
      const accountId = randomUUID()
      const deliveries = Array.from({ length: 10 }, (_, index) =>
        Effect.gen(function* () {
          const subscriptions = yield* Subscriptions.Subscriptions
          return yield* subscriptions.withAccountLock(
            accountId,
            Effect.gen(function* () {
              yield* Effect.sleep("50 millis")
              yield* subscriptions.record(
                subscription(accountId, `2026-11-04T10:00:${String(index).padStart(2, "0")}Z`)
              )
              return (yield* subscriptions.ofAccount(accountId, undefined)).length
            })
          )
        })
      )

      const results = await services.runPromise(Effect.all(deliveries, { concurrency: "unbounded" }))

      expect(results).toEqual(Array.from({ length: 10 }, () => 1))
    } finally {
      await services.dispose()
    }
  })
})
