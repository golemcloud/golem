import { Effect, Layer } from "effect"
import { TestClock } from "effect/testing"
import { describe, expect, it } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import * as Registry from "../src/Registry.ts"
import { ORDER_PAYMENT_GRACE_MS, planFor, SETTLEMENT_MARGIN_MS, sweeper } from "../src/Settlement.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import type { PendingOrder, Subscription } from "../src/Subscriptions.ts"
import { polarProductId, testConfig } from "./support.ts"

const now = new Date("2026-11-20T12:00:00Z")
const MINUTE = 60 * 1000
const PAST_DUE_GRACE_MS = 7 * 24 * 60 * MINUTE

const subscription = (overrides: Partial<Subscription> = {}): Subscription => ({
  subscriptionId: "sub_1",
  accountId: "a0a4c3f2-6f43-4a5e-9a51-8c3f0d5b7e21",
  productId: "product",
  status: "active",
  currentPeriodEnd: new Date("2026-12-04T10:00:00Z"),
  cancelAtPeriodEnd: false,
  modifiedAt: new Date("2026-11-04T10:00:00Z"),
  meteredPriceIds: ["price_compute"],
  pastDueAt: undefined,
  ...overrides
})

const endingIn = (milliseconds: number) =>
  subscription({ cancelAtPeriodEnd: true, currentPeriodEnd: new Date(now.getTime() + milliseconds) })

const pending = (overrides: Partial<PendingOrder> = {}): PendingOrder => ({
  orderId: "order_1",
  subscriptionId: "sub_1",
  createdAt: new Date(now.getTime() - MINUTE),
  meteredAmount: 345,
  ...overrides
})

const priceOnly = pending({ meteredAmount: 0 })
const pastDue = subscription({ status: "past_due", pastDueAt: new Date(now.getTime() - MINUTE) })
const pastDueAfterGrace = subscription({
  status: "past_due",
  pastDueAt: new Date(now.getTime() - PAST_DUE_GRACE_MS - MINUTE)
})
const revokedUnpaid = subscription({ status: "canceled", pastDueAt: new Date(now.getTime() - MINUTE) })
const overdue = pending({ createdAt: new Date(now.getTime() - ORDER_PAYMENT_GRACE_MS - MINUTE) })

describe("planFor", () => {
  it.each([
    ["no subscription", "free", [], []],
    ["an active subscription", "paid", [subscription()], []],
    ["a subscription that ends after the settlement margin", "paid", [endingIn(SETTLEMENT_MARGIN_MS + MINUTE)], []],
    ["a subscription that ends within the settlement margin", "free", [endingIn(SETTLEMENT_MARGIN_MS - MINUTE)], []],
    ["a subscription past its scheduled end", "free", [endingIn(-MINUTE)], []],
    ["a past_due subscription in the past-due grace, with an unpaid price", "paid", [pastDue], [priceOnly]],
    ["a past_due subscription in the past-due grace, with unpaid overage", "paid", [pastDue], [pending()]],
    ["a past_due subscription in the past-due grace, with overdue overage", "paid", [pastDue], [overdue]],
    ["a past_due subscription after the past-due grace, with an unpaid price", "free", [pastDueAfterGrace], [priceOnly]],
    ["a past_due subscription after the past-due grace, with unpaid overage", "suspended", [pastDueAfterGrace], [pending()]],
    ["a subscription revoked after a failed payment, with unpaid overage", "suspended", [revokedUnpaid], [pending()]],
    ["an active subscription while Polar charges its overage", "paid", [subscription()], [pending()]],
    ["an active subscription with overdue overage", "suspended", [subscription()], [overdue]],
    ["a canceled subscription with overdue overage", "suspended", [subscription({ status: "canceled" })], [overdue]],
    ["a canceled subscription with an overdue price", "free", [subscription({ status: "canceled" })], [pending({ ...overdue, meteredAmount: 0 })]],
    ["a canceled subscription", "free", [subscription({ status: "canceled" })], []],
    ["a trialing subscription", "free", [subscription({ status: "trialing" })], []],
    ["a paused subscription", "free", [subscription({ status: "paused" })], []],
    ["an incomplete subscription", "free", [subscription({ status: "incomplete" })], []],
    [
      "a new active subscription with overdue overage of the old one",
      "suspended",
      [subscription({ status: "canceled" }), subscription({ subscriptionId: "sub_2" })],
      [overdue]
    ]
  ] as const)("%s gives the plan %s", (_, plan, subscriptions, orders) => {
    expect(planFor(subscriptions, orders, now, PAST_DUE_GRACE_MS)).toBe(plan)
  })

  it("gives no past-due grace when its length is 0", () => {
    expect(planFor([pastDue], [priceOnly], now, 0)).toBe("free")
    expect(planFor([pastDue], [pending()], now, 0)).toBe("suspended")
  })
})

describe("sweeper", () => {
  it("runs the sweep when the web app starts and then every minute", async () => {
    const sweeps: Array<string> = []
    const subscriptions = Layer.succeed(Subscriptions.Subscriptions, {
      ending: (productId: string) => Effect.sync(() => (sweeps.push(productId), [])),
      awaitingPayment: () => Effect.succeed([]),
      pastDue: () => Effect.succeed([]),
      record: () => Effect.die("unused"),
      recordOrder: () => Effect.die("unused"),
      pendingOrders: () => Effect.die("unused"),
      ofAccount: () => Effect.die("unused"),
      latest: () => Effect.die("unused"),
      isDelivered: () => Effect.die("unused"),
      markDelivered: () => Effect.die("unused"),
      withAccountLock: () => Effect.die("unused")
    })
    const services = Layer.mergeAll(
      subscriptions,
      Layer.succeed(Registry.Registry, {} as Registry.RegistryShape),
      Layer.succeed(BillingConfig, testConfig())
    )

    await Effect.runPromise(
      Effect.gen(function* () {
        yield* Layer.build(sweeper.pipe(Layer.provide(services)))
        yield* Effect.yieldNow
        expect(sweeps).toHaveLength(1)
        yield* TestClock.adjust("1 minute")
        expect(sweeps).toHaveLength(2)
        yield* TestClock.adjust("59 seconds")
        expect(sweeps).toHaveLength(2)
        yield* TestClock.adjust("1 second")
        expect(sweeps).toEqual([polarProductId, polarProductId, polarProductId])
      }).pipe(Effect.scoped, Effect.provide(TestClock.layer()))
    )
  })
})
