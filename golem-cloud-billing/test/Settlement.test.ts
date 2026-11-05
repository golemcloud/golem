import { Effect, Layer } from "effect"
import { TestClock } from "effect/testing"
import { describe, expect, it } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import * as Registry from "../src/Registry.ts"
import { planFor, SETTLEMENT_MARGIN_MS, sweeper } from "../src/Settlement.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import type { Subscription } from "../src/Subscriptions.ts"
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
  pastDueAt: undefined,
  ...overrides
})

const endingIn = (milliseconds: number) =>
  subscription({ cancelAtPeriodEnd: true, currentPeriodEnd: new Date(now.getTime() + milliseconds) })

describe("planFor", () => {
  it.each([
    ["no subscription", "free", []],
    ["an active subscription", "paid", [subscription()]],
    ["a subscription that ends after the settlement margin", "paid", [endingIn(SETTLEMENT_MARGIN_MS + MINUTE)]],
    ["a subscription that ends within the settlement margin", "free", [endingIn(SETTLEMENT_MARGIN_MS - MINUTE)]],
    ["a subscription past its scheduled end", "free", [endingIn(-MINUTE)]],
    ["a past_due subscription", "suspended", [subscription({ status: "past_due", pastDueAt: now })]],
    ["an unpaid subscription", "suspended", [subscription({ status: "unpaid" })]],
    ["a subscription that Polar revoked while it was unpaid", "suspended", [subscription({ status: "canceled", pastDueAt: now })]],
    ["a canceled subscription", "free", [subscription({ status: "canceled" })]],
    ["a trialing subscription", "free", [subscription({ status: "trialing" })]],
    ["a paused subscription", "free", [subscription({ status: "paused" })]],
    ["an incomplete subscription", "free", [subscription({ status: "incomplete" })]],
    [
      "an unpaid revoked and a new active subscription",
      "paid",
      [subscription({ status: "canceled", pastDueAt: now }), subscription({ subscriptionId: "sub_2" })]
    ]
  ] as const)("%s gives the plan %s", (_, plan, subscriptions) => {
    expect(planFor(subscriptions, now)).toBe(plan)
  })
})

describe("sweeper", () => {
  it("runs the sweep when the web app starts and then every minute", async () => {
    const sweeps: Array<string> = []
    const subscriptions = Layer.succeed(Subscriptions.Subscriptions, {
      ending: (productId: string) => Effect.sync(() => (sweeps.push(productId), [])),
      record: () => Effect.die("unused"),
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
