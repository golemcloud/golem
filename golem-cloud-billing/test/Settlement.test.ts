import { Effect, Layer } from "effect"
import { TestClock } from "effect/testing"
import { describe, expect, it } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import { currencies } from "../src/Currency.ts"
import * as Polar from "../src/Polar.ts"
import * as Registry from "../src/Registry.ts"
import * as RegistryAccounts from "../src/RegistryAccounts.ts"
import {
  holdFor,
  holdReasonText,
  type HoldReason,
  ORDER_PAYMENT_GRACE_MS,
  planFor,
  SETTLEMENT_MARGIN_MS,
  SETTLEMENT_MAX_ORDERS,
  SETTLEMENT_MAXIMUM,
  SETTLEMENT_MINIMUM,
  CONTACT_EMAIL,
  settlementFor,
  sweeper
} from "../src/Settlement.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import type { Checkout, CheckoutStatus, Subscription, UnpaidOrder } from "../src/Subscriptions.ts"
import { polarProductId, testConfig } from "./support.ts"

const now = new Date("2026-11-20T12:00:00Z")
const MINUTE = 60 * 1000
const PAST_DUE_GRACE_MS = 7 * 24 * 60 * MINUTE

const subscription = (overrides: Partial<Subscription> = {}): Subscription => ({
  subscriptionId: "sub_1",
  accountId: "a0a4c3f2-6f43-4a5e-9a51-8c3f0d5b7e21",
  productId: "product",
  currency: "usd",
  status: "active",
  currentPeriodEnd: new Date("2026-12-04T10:00:00Z"),
  cancelAtPeriodEnd: false,
  endedAt: undefined,
  modifiedAt: "2026-11-04T10:00:00Z",
  meteredPriceIds: ["price_compute"],
  pastDueAt: undefined,
  ...overrides
})

const endingIn = (milliseconds: number) =>
  subscription({ cancelAtPeriodEnd: true, currentPeriodEnd: new Date(now.getTime() + milliseconds) })

/** A subscription canceled at the period end whose payment failed a minute ago: in the past-due grace. */
const pastDueEndingIn = (milliseconds: number) =>
  subscription({ ...endingIn(milliseconds), status: "past_due", pastDueAt: new Date(now.getTime() - MINUTE) })

const pending = (overrides: Partial<UnpaidOrder> = {}): UnpaidOrder => ({
  orderId: "order_1",
  subscriptionId: "sub_1",
  currency: "usd",
  status: "pending",
  createdAt: new Date(now.getTime() - MINUTE),
  meteredTotal: 345,
  ...overrides
})

const priceOnly = pending({ meteredTotal: 0 })
const pastDue = subscription({ status: "past_due", pastDueAt: new Date(now.getTime() - MINUTE) })
const pastDueAfterGrace = subscription({
  status: "past_due",
  pastDueAt: new Date(now.getTime() - PAST_DUE_GRACE_MS - MINUTE)
})
const revokedUnpaid = subscription({ status: "canceled", pastDueAt: new Date(now.getTime() - MINUTE) })
const overdue = pending({ createdAt: new Date(now.getTime() - ORDER_PAYMENT_GRACE_MS - MINUTE) })
const ended = subscription({ status: "canceled", endedAt: new Date(now.getTime() - 2 * ORDER_PAYMENT_GRACE_MS) })
const belowMinimum = { meteredTotal: SETTLEMENT_MINIMUM.usd - 1 }

describe("planFor", () => {
  it.each([
    ["no subscription", "free", [], []],
    ["an active subscription", "paid", [subscription()], []],
    ["a subscription that ends after the settlement margin", "paid", [endingIn(SETTLEMENT_MARGIN_MS + MINUTE)], []],
    ["a subscription that ends within the settlement margin", "free", [endingIn(SETTLEMENT_MARGIN_MS - MINUTE)], []],
    ["a subscription past its scheduled end", "free", [endingIn(-MINUTE)], []],
    [
      "a past_due subscription in the past-due grace that ends after the settlement margin",
      "paid",
      [pastDueEndingIn(SETTLEMENT_MARGIN_MS + MINUTE)],
      []
    ],
    [
      "a past_due subscription in the past-due grace that ends within the settlement margin",
      "free",
      [pastDueEndingIn(SETTLEMENT_MARGIN_MS - MINUTE)],
      []
    ],
    ["a past_due subscription in the past-due grace past its scheduled end", "free", [pastDueEndingIn(-MINUTE)], []],
    [
      "a past_due subscription in the past-due grace that ends within the settlement margin, with overdue overage of an older one",
      "suspended",
      [subscription({ subscriptionId: "sub_0", status: "canceled" }), pastDueEndingIn(SETTLEMENT_MARGIN_MS - MINUTE)],
      [pending({ ...overdue, subscriptionId: "sub_0" })]
    ],
    ["a past_due subscription in the past-due grace, with an unpaid price", "paid", [pastDue], [priceOnly]],
    ["a past_due subscription in the past-due grace, with unpaid overage", "paid", [pastDue], [pending()]],
    ["a past_due subscription in the past-due grace, with overdue overage", "paid", [pastDue], [overdue]],
    ["a past_due subscription after the past-due grace, with an unpaid price", "free", [pastDueAfterGrace], [priceOnly]],
    ["a past_due subscription after the past-due grace, with unpaid overage", "suspended", [pastDueAfterGrace], [pending()]],
    ["a subscription revoked after a failed payment, with unpaid overage", "suspended", [revokedUnpaid], [pending()]],
    ["an active subscription while Polar charges its overage", "paid", [subscription()], [pending()]],
    ["an active subscription with overdue overage", "suspended", [subscription()], [overdue]],
    ["a canceled subscription with overdue overage", "suspended", [subscription({ status: "canceled" })], [overdue]],
    ["a canceled subscription with an overdue price", "free", [subscription({ status: "canceled" })], [pending({ ...overdue, meteredTotal: 0 })]],
    ["a canceled subscription", "free", [subscription({ status: "canceled" })], []],
    ["a revoked subscription whose unpaid order Polar voided", "suspended", [revokedUnpaid], [pending({ status: "void" })]],
    ["an ended subscription with overdue overage below the settlement minimum", "free", [ended], [pending({ ...overdue, ...belowMinimum })]],
    [
      "an ended subscription with two overdue orders below the settlement minimum, which reach it together",
      "free",
      [ended],
      [pending({ ...overdue, meteredTotal: 25 }), pending({ ...overdue, orderId: "order_2", meteredTotal: 25 })]
    ],
    [
      "a past_due subscription after the past-due grace, with overage below the settlement minimum",
      "suspended",
      [pastDueAfterGrace],
      [pending(belowMinimum)]
    ],
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
    ["an active subscription with unbilled usage", ["subscription", "unbilledUsage"], [subscription()], [], true],
    [
      "a subscription canceled at the period end with unbilled usage",
      ["canceledSubscription", "unbilledUsage"],
      [canceledAtPeriodEnd],
      [],
      true
    ],
    ["an ended subscription with unbilled usage", ["unsettledUsage"], [subscription({ status: "canceled" })], [], true],
    ["unbilled usage without a subscription", ["unsettledUsage"], [], [], true],
    [
      "an ended subscription with unbilled usage next to a billable one",
      ["subscription", "unbilledUsage"],
      [subscription({ status: "canceled" }), subscription({ subscriptionId: "sub_2" })],
      [],
      true
    ],
    ["an unpaid order with overage", ["unpaidOverage"], [subscription({ status: "canceled" })], [pending()], false],
    ["an unpaid order of the price alone", [], [subscription({ status: "canceled" })], [priceOnly], false],
    ["a void order with overage", ["unpaidOverage"], [revokedUnpaid], [pending({ status: "void" })], false],
    ["an order of an ended subscription with overage below the settlement minimum", [], [ended], [pending(belowMinimum)], false],
    [
      "two orders of an ended subscription below the settlement minimum, which reach it together",
      [],
      [ended],
      [pending(belowMinimum), pending({ ...belowMinimum, orderId: "order_2" })],
      false
    ],
    [
      "an active subscription with every other reason",
      ["subscription", "unbilledUsage", "unpaidOverage"],
      [subscription()],
      [overdue],
      true
    ]
  ] as const)("%s gives %j", (_, reasons, subscriptions, orders, unbilledUsage) => {
    expect(holdFor(subscriptions, orders, unbilledUsage, [])).toEqual(reasons)
  })

  const checkout = (status: CheckoutStatus, expiresAt = new Date("2026-11-21T12:00:00Z")): Checkout => ({
    checkoutId: `checkout_${status}_${expiresAt.getTime()}`,
    accountId: subscription().accountId,
    status,
    expiresAt
  })

  it.each([
    ["an open checkout", ["openCheckout"], [], [checkout("open")]],
    ["a confirmed checkout", ["paidCheckout"], [], [checkout("confirmed")]],
    ["a succeeded checkout", ["paidCheckout"], [], [checkout("succeeded")]],
    ["an open and a succeeded checkout", ["openCheckout", "paidCheckout"], [], [checkout("open"), checkout("succeeded")]],
    ["an active subscription and an open checkout", ["subscription", "openCheckout"], [subscription()], [checkout("open")]]
  ] as const)("%s gives %j", (_, reasons, subscriptions, checkouts) => {
    expect(holdFor(subscriptions, [], false, checkouts)).toEqual(reasons)
  })

  it("names when the delete works: 30 minutes after the open checkout that expires last", () => {
    const later = checkout("open", new Date("2026-11-21T15:30:00Z"))
    const text = (now: string) =>
      holdReasonText(["openCheckout"], "https://billing.golem.cloud", [checkout("open"), later], new Date(now))

    expect(text("2026-11-21T15:59:00Z")).toBe(
      "Golem Cloud billing: A checkout for Pro from the billing page is open. Delete the account after November 21, 2026 at 4:00 PM UTC."
    )
    // The hold stays until Polar reports the checkout `expired`, also after that time.
    expect(text("2026-11-21T16:00:00Z")).toBe(
      `Golem Cloud billing: A checkout for Pro from the billing page is past its expiry, and Polar has not closed it yet. Try again in an hour. If the delete still fails, contact Golem Cloud at ${CONTACT_EMAIL}.`
    )
  })

  it("writes the longest reasons that holdFor gives within the 1000 characters of a registry hold", () => {
    // holdFor gives `subscription` or `canceledSubscription`, and `unbilledUsage` or
    // `unsettledUsage`; each first one is the longer text. The text of `openCheckout` is longer
    // after the time that it names.
    const longest: ReadonlyArray<HoldReason> = [
      "subscription",
      "unbilledUsage",
      "unpaidOverage",
      "openCheckout",
      "paidCheckout"
    ]
    const text = holdReasonText(
      longest,
      "https://billing.golem.cloud",
      [checkout("open"), checkout("succeeded")],
      new Date("2026-11-22T12:00:00Z")
    )

    expect(text).toContain("Polar has not closed it yet.")
    expect(text.length).toBeLessThanOrEqual(1000)
    expect(text).toMatch(/^Golem Cloud billing: The account has a subscription to Pro\. Cancel it in Manage billing at https:\/\/billing\.golem\.cloud\. The subscription then runs to the end of the period\. Delete the account after the period ends\./)
  })

  it("promises an invoice only for the overage of a billable subscription, and names a contact for the rest", () => {
    const billingUrl = "https://billing.golem.cloud"

    expect(holdReasonText(["subscription", "unbilledUsage"], billingUrl, [], new Date())).toContain(
      "Polar bills it on the invoice at the end of the period."
    )
    const unsettled = holdReasonText(["unsettledUsage"], billingUrl, [], new Date())
    expect(unsettled).toBe(
      `Golem Cloud billing: The billing of the paid overage of the account is not final yet. Try again in an hour. If the delete still fails, contact Golem Cloud at ${CONTACT_EMAIL}.`
    )
    expect(unsettled).not.toMatch(/invoice|period/)
  })
})

describe("settlementFor", () => {
  it("offers no settlement while Polar collects the order", () => {
    expect(settlementFor([subscription()], [overdue])).toBeUndefined()
    expect(settlementFor([pastDueAfterGrace], [pending()])).toBeUndefined()
  })

  it("settles the overage of an order of an ended subscription, without its price", () => {
    expect(settlementFor([ended], [overdue])).toEqual({
      currency: "usd",
      orders: [{ orderId: "order_1", amount: 345 }],
      amount: 345
    })
    expect(settlementFor([ended], [priceOnly])).toBeUndefined()
  })

  it("settles a void order, also before the webhook of the revoke arrives", () => {
    expect(settlementFor([subscription()], [pending({ status: "void" })])).toEqual({
      currency: "usd",
      orders: [{ orderId: "order_1", amount: 345 }],
      amount: 345
    })
  })

  it("offers no settlement below the settlement minimum", () => {
    expect(settlementFor([ended], [pending(belowMinimum)])).toBeUndefined()
    expect(settlementFor([ended], [pending(belowMinimum), pending({ ...belowMinimum, orderId: "order_2" })])).toBeUndefined()
  })

  it("keeps a settlement within the maximum price of Polar, and skips an order that does not fit", () => {
    const maximum = SETTLEMENT_MAXIMUM.usd
    const large = maximum - 100_000

    expect(
      settlementFor([ended], [pending({ meteredTotal: large }), pending({ orderId: "order_2", meteredTotal: large })])
    ).toEqual({ currency: "usd", orders: [{ orderId: "order_1", amount: large }], amount: large })
    expect(
      settlementFor([ended], [pending({ meteredTotal: maximum + 1 }), pending({ orderId: "order_2" })])
    ).toEqual({ currency: "usd", orders: [{ orderId: "order_2", amount: 345 }], amount: 345 })
    expect(settlementFor([ended], [pending({ meteredTotal: maximum })])).toEqual({
      currency: "usd",
      orders: [{ orderId: "order_1", amount: maximum }],
      amount: maximum
    })
  })

  it("leaves out the orders below the settlement minimum, also when they would fill a batch before a payable order", () => {
    const small = Array.from({ length: SETTLEMENT_MAX_ORDERS }, (_, index) =>
      pending({ orderId: `order_small_${index}`, meteredTotal: 1 })
    )
    const payable = pending({ orderId: "order_payable", meteredTotal: SETTLEMENT_MINIMUM.usd })

    expect(settlementFor([ended], [...small, payable])).toEqual({
      currency: "usd",
      orders: [{ orderId: "order_payable", amount: SETTLEMENT_MINIMUM.usd }],
      amount: SETTLEMENT_MINIMUM.usd
    })
    expect(planFor([ended], [...small, { ...payable, createdAt: overdue.createdAt }], now, PAST_DUE_GRACE_MS)).toBe("suspended")
  })

  it("settles only the orders that Polar no longer collects", () => {
    const orders = [pending({ subscriptionId: "sub_2" }), pending({ orderId: "order_2", status: "void", meteredTotal: 100 })]

    expect(settlementFor([subscription({ subscriptionId: "sub_2" })], orders)).toEqual({
      currency: "usd",
      orders: [{ orderId: "order_2", amount: 100 }],
      amount: 100
    })
  })

  it("settles at most the oldest orders whose IDs fit in the metadata of Polar", () => {
    const orders = Array.from({ length: SETTLEMENT_MAX_ORDERS + 2 }, (_, index) =>
      pending({ orderId: `0b7a4b6e-8c2b-4f63-bb0e-6a4cf7c1b9${String(index).padStart(2, "0")}` })
    )

    const offer = settlementFor([ended], orders)

    expect(offer?.orders).toHaveLength(SETTLEMENT_MAX_ORDERS)
    expect(offer?.orders[0]?.orderId).toBe(orders[0]?.orderId)
    expect(offer?.amount).toBe(SETTLEMENT_MAX_ORDERS * 345)
    expect(offer!.orders.map((order) => order.orderId).join(",").length).toBeLessThanOrEqual(500)
  })

  it("settles only the orders in the currency of the oldest order, and leaves the others for a later settlement", () => {
    const orders = [
      pending({ orderId: "order_1" }),
      pending({ orderId: "order_2", currency: "gbp", meteredTotal: 280 }),
      pending({ orderId: "order_3", meteredTotal: 100 })
    ]

    expect(settlementFor([ended], orders)).toEqual({
      currency: "usd",
      orders: [
        { orderId: "order_1", amount: 345 },
        { orderId: "order_3", amount: 100 }
      ],
      amount: 445
    })
    expect(settlementFor([ended], orders.slice(1, 2))).toEqual({
      currency: "gbp",
      orders: [{ orderId: "order_2", amount: 280 }],
      amount: 280
    })
  })

  it("takes the currency of the oldest order that fits within the maximum", () => {
    const orders = [
      pending({ orderId: "order_1", currency: "eur", meteredTotal: SETTLEMENT_MAXIMUM.eur + 1 }),
      pending({ orderId: "order_2", currency: "gbp" }),
      pending({ orderId: "order_3", currency: "eur" })
    ]

    expect(settlementFor([ended], orders)).toEqual({
      currency: "gbp",
      orders: [{ orderId: "order_2", amount: 345 }],
      amount: 345
    })
  })

  it.each(currencies)("writes off an order in %s below the settlement minimum of its currency", (currency) => {
    const minimum = SETTLEMENT_MINIMUM[currency]
    const below = pending({ ...overdue, currency, meteredTotal: minimum - 1 })
    const atMinimum = pending({ ...overdue, currency, meteredTotal: minimum })

    expect(settlementFor([ended], [below])).toBeUndefined()
    expect(planFor([ended], [below], now, PAST_DUE_GRACE_MS)).toBe("free")
    expect(holdFor([ended], [below], false, [])).toEqual([])
    expect(settlementFor([ended], [atMinimum])).toEqual({
      currency,
      orders: [{ orderId: "order_1", amount: minimum }],
      amount: minimum
    })
    expect(planFor([ended], [atMinimum], now, PAST_DUE_GRACE_MS)).toBe("suspended")
  })
})

describe("sweeper", () => {
  it("runs the sweep when the web app starts and then every minute", async () => {
    const sweeps: Array<string> = []
    const subscriptions = Layer.succeed(Subscriptions.Subscriptions, {
      ending: (productId: string) => Effect.sync(() => (sweeps.push(productId), [])),
      awaitingPayment: () => Effect.succeed([]),
      pastDue: () => Effect.succeed([]),
      holdCandidates: () => Effect.succeed([]),
      recordHold: () => Effect.die("unused"),
      forgetHold: () => Effect.die("unused"),
      record: () => Effect.die("unused"),
      recordOrder: () => Effect.die("unused"),
      unpaidOrders: () => Effect.die("unused"),
      recordSettlement: () => Effect.die("unused"),
      settlementOrderOf: () => Effect.die("unused"),
      unbilledUsage: () => Effect.die("unused"),
      ofAccount: () => Effect.die("unused"),
      latest: () => Effect.die("unused"),
      recordCheckout: () => Effect.die("unused"),
      unfinishedCheckouts: () => Effect.die("unused"),
      linkCheckout: () => Effect.die("unused"),
      checkoutsToRead: () => Effect.succeed([]),
      recordCheckoutStatus: () => Effect.die("unused"),
      isDelivered: () => Effect.die("unused"),
      markDelivered: () => Effect.die("unused"),
      withAccountLock: () => Effect.die("unused")
    })
    const services = Layer.mergeAll(
      subscriptions,
      Layer.succeed(Registry.Registry, {} as Registry.RegistryShape),
      Layer.succeed(Polar.Polar, {} as Polar.PolarShape),
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
