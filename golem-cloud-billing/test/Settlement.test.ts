import { Effect, Layer } from "effect"
import { TestClock } from "effect/testing"
import { describe, expect, it } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import * as Registry from "../src/Registry.ts"
import * as RegistryAccounts from "../src/RegistryAccounts.ts"
import {
  holdFor,
  holdReasonText,
  type HoldReason,
  ORDER_PAYMENT_GRACE_MS,
  planFor,
  SETTLEMENT_MARGIN_MS,
  sweeper
} from "../src/Settlement.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import type { PendingOrder, Subscription } from "../src/Subscriptions.ts"
import { polarProductId, testConfig } from "./support.ts"

const now = new Date("2026-11-20T12:00:00Z")
const MINUTE = 60 * 1000

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
const revokedUnpaid = subscription({ status: "canceled", pastDueAt: new Date(now.getTime() - MINUTE) })
const overdue = pending({ createdAt: new Date(now.getTime() - ORDER_PAYMENT_GRACE_MS - MINUTE) })

describe("planFor", () => {
  it.each([
    ["no subscription", "free", [], []],
    ["an active subscription", "paid", [subscription()], []],
    ["a subscription that ends after the settlement margin", "paid", [endingIn(SETTLEMENT_MARGIN_MS + MINUTE)], []],
    ["a subscription that ends within the settlement margin", "free", [endingIn(SETTLEMENT_MARGIN_MS - MINUTE)], []],
    ["a subscription past its scheduled end", "free", [endingIn(-MINUTE)], []],
    ["a past_due subscription with an unpaid price", "free", [pastDue], [priceOnly]],
    ["a past_due subscription with unpaid overage", "suspended", [pastDue], [pending()]],
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
    expect(planFor(subscriptions, orders, now)).toBe(plan)
  })
})

const canceledAtPeriodEnd = subscription({ cancelAtPeriodEnd: true })

describe("holdFor", () => {
  it.each([
    ["no subscription", [], [], [], false],
    ["an active subscription", ["subscription"], [subscription()], [], false],
    ["a trialing subscription", ["subscription"], [subscription({ status: "trialing" })], [], false],
    ["a past_due subscription", ["subscription"], [pastDue], [], false],
    ["a subscription canceled at the period end", ["canceledSubscription"], [canceledAtPeriodEnd], [], false],
    [
      "a canceled subscription next to one that continues",
      ["subscription"],
      [canceledAtPeriodEnd, subscription({ subscriptionId: "sub_2" })],
      [],
      false
    ],
    ["an ended subscription", [], [subscription({ status: "canceled" })], [], false],
    ["a paused subscription", [], [subscription({ status: "paused" })], [], false],
    ["an ended subscription with unbilled usage", ["unbilledUsage"], [subscription({ status: "canceled" })], [], true],
    ["an unpaid order with overage", ["unpaidOverage"], [subscription({ status: "canceled" })], [pending()], false],
    ["an unpaid order of the price alone", [], [subscription({ status: "canceled" })], [priceOnly], false],
    [
      "an active subscription with every other reason",
      ["subscription", "unbilledUsage", "unpaidOverage"],
      [subscription()],
      [overdue],
      true
    ]
  ] as const)("%s gives %j", (_, reasons, subscriptions, orders, unbilledUsage) => {
    expect(holdFor(subscriptions, orders, unbilledUsage)).toEqual(reasons)
  })

  it("writes every reason within the 1000 characters of a registry hold", () => {
    const every: ReadonlyArray<HoldReason> = ["subscription", "canceledSubscription", "unbilledUsage", "unpaidOverage"]
    const text = holdReasonText(every, "https://billing.golem.cloud")

    expect(text.length).toBeLessThanOrEqual(1000)
    expect(text).toMatch(/^Golem Cloud billing: The account has a subscription to Paid\. Cancel it in Manage billing at https:\/\/billing\.golem\.cloud\. The subscription then runs to the end of the period\. Delete the account after the period ends\./)
  })
})

describe("sweeper", () => {
  it("runs the sweep when the web app starts and then every minute", async () => {
    const sweeps: Array<string> = []
    const subscriptions = Layer.succeed(Subscriptions.Subscriptions, {
      ending: (productId: string) => Effect.sync(() => (sweeps.push(productId), [])),
      awaitingPayment: () => Effect.succeed([]),
      holdCandidates: () => Effect.succeed([]),
      recordHold: () => Effect.die("unused"),
      forgetHold: () => Effect.die("unused"),
      record: () => Effect.die("unused"),
      recordOrder: () => Effect.die("unused"),
      pendingOrders: () => Effect.die("unused"),
      unbilledUsage: () => Effect.die("unused"),
      ofAccount: () => Effect.die("unused"),
      latest: () => Effect.die("unused"),
      isDelivered: () => Effect.die("unused"),
      markDelivered: () => Effect.die("unused"),
      withAccountLock: () => Effect.die("unused")
    })
    const services = Layer.mergeAll(
      subscriptions,
      Layer.succeed(Registry.Registry, {} as Registry.RegistryShape),
      Layer.succeed(RegistryAccounts.RegistryAccounts, {
        isDeleted: () => Effect.die("unused"),
        deletedAmong: () => Effect.succeed([])
      }),
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
