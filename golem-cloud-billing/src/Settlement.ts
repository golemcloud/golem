import { Effect, Layer, Option, Ref, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import type { Currency, PerCurrency } from "./Currency.ts"
import { Polar } from "./Polar.ts"
import { type Account, Registry, type RegistryError } from "./Registry.ts"
import { RegistryAccounts, type RegistryAccountsError } from "./RegistryAccounts.ts"
import {
  billableStatuses,
  type Checkout,
  type CheckoutStatus,
  type Subscription,
  Subscriptions,
  type SubscriptionsError,
  type UnpaidOrder
} from "./Subscriptions.ts"

/**
 * Settlement: the plan of an account follows what the account has paid.
 *
 * The paid plan is prepaid with optional postpay: its price buys the monthly amounts in advance,
 * and Polar bills the paid overage after the period, on the order of the next renewal or on the
 * last order after a cancellation. Polar bills a usage event only if the customer has a billable
 * subscription when Polar processes the event, and it skips the events that arrive later. It
 * bills no usage during a trial.
 *
 * - `suspended`: the account did not pay for paid overage. An order that the account owes
 *   (`owedOrders`) is not in the past-due grace of its subscription, and a payment of the
 *   subscription failed (`past_due_at`, which stays after Polar revokes the subscription) or the
 *   order is older than `ORDER_PAYMENT_GRACE_MS`. The plan has no monthly amounts and no overage,
 *   so the agents of the account stop until the order is paid or settled. A new subscription does
 *   not end the suspension.
 * - `paid`: a subscription is `active` or in the past-due grace, and does not end within
 *   `SETTLEMENT_MARGIN_MS`. Only this plan has paid overage.
 * - `free`: otherwise. A declined payment of the price alone after the past-due grace, a
 *   cancellation, a pause and a trial give the free plan. A subscription that is canceled at the
 *   period end moves the account to free `SETTLEMENT_MARGIN_MS` before the end, so the usage push
 *   sends the last paid overage while Polar still bills it.
 *
 * The past-due grace: a subscription is `past_due`, and its first failed payment is less than
 * `pastDueGraceMs` ago. Polar retries the payment in this time. The account keeps the paid plan,
 * so it uses its monthly amounts and its paid overage as before. Polar bills the usage of a
 * `past_due` subscription, so the paid overage of the past-due grace goes on the next invoice. A
 * subscription that Polar revokes is `canceled` and has no past-due grace.
 *
 * An unpaid order (`Subscriptions.UnpaidOrder`) is `pending`, or `void` with no settlement. Polar
 * collects a `pending` order of a billable subscription: it retries the payment, and a new card in
 * the customer portal pays it. Polar collects no other unpaid order. It does not retry an order of
 * an ended subscription, and it voids the pending orders of a subscription that it revokes, also
 * after the failed retries of a payment. A void collects nothing, so the app keeps its own record
 * of what the account owes and never lets a void remove a debt. The person pays such orders with a
 * settlement: a checkout of the settlement product (`POLAR_SETTLEMENT_PRODUCT_ID`) for their paid
 * overage, whose `order.paid` records the settlement of each order (`settlementFor`).
 *
 * The registry sets an account on a plan without overage to hardLimit. Only the owner can turn
 * paid overage on again, also after the account is back on the paid plan.
 *
 * Settlement also places a deletion hold on the account while the account has a billable
 * subscription or owes paid overage (`holdFor`), so the registry refuses to delete it. Golem does
 * not charge early and does not end a paid period early: the person deletes the account after the
 * subscription ends and Polar has billed the paid overage and been paid for it. When Polar ends a
 * subscription before its period end, it never bills the paid overage after the newest order, and
 * that overage does not hold the account.
 *
 * A checkout of the paid product also holds the account, from before the person gets its URL until
 * the app records the subscription that it started, or Polar reports that it expired or failed.
 * Polar takes the payment of a checkout for 24 hours, and no API closes a checkout earlier, so
 * without the hold a payment could start a subscription of a deleted account.
 */

/**
 * How long before the scheduled end of a subscription the account moves to the free plan. It is
 * longer than the interval of the usage push and the usage report interval of the executors
 * together, so the push sends the last paid overage before the subscription ends.
 */
export const SETTLEMENT_MARGIN_MS = 15 * 60 * 1000

/**
 * How long an order can stay `pending` before an unpaid amount for overage suspends the account.
 * Polar charges an order when it creates it, so a paid order is `paid` well within this time. For
 * a subscription that has ended, Polar does not retry a failed charge and does not mark the
 * subscription `past_due`: only the age of the order shows the failure.
 */
export const ORDER_PAYMENT_GRACE_MS = 60 * 60 * 1000

/**
 * The smallest amount of a settlement in each currency, in minor units: $0.50, €0.50 and £0.50.
 * Polar refuses a fixed price below $0.50, €0.50 or £0.40, so a checkout cannot settle a smaller
 * amount. The app writes off an order that Polar no longer collects when its paid overage is
 * smaller than the minimum of its currency (see `owedOrders`).
 */
export const SETTLEMENT_MINIMUM: PerCurrency<number> = { usd: 50, eur: 50, gbp: 50 }

/**
 * The largest amount of a settlement in each currency, in minor units: the largest fixed price of
 * Polar, 999,999.99 in each. A settlement stays within it. A checkout cannot settle an order above
 * it; an operator handles such an order.
 */
export const SETTLEMENT_MAXIMUM: PerCurrency<number> = { usd: 99_999_999, eur: 99_999_999, gbp: 99_999_999 }

/**
 * The most orders that one settlement pays. Polar keeps a metadata value of at most 500
 * characters, which holds 13 order IDs. A later settlement pays the remaining orders. An account
 * rarely owes more than one, because a suspended account cannot subscribe again before it settles.
 */
export const SETTLEMENT_MAX_ORDERS = 13

/**
 * How old a recorded deletion hold gets before the sweep reconciles the account again. A placement
 * refreshes the record, so a held account is checked about once in this time. A removal that
 * failed keeps the old record, so the sweep tries it again, long after the last webhook.
 */
export const HOLD_RECHECK_MS = 60 * 60 * 1000

/** How long after the expiry of a checkout the sweep first reads the checkout from Polar. */
export const CHECKOUT_READ_DELAY_MS = 5 * 60 * 1000

/**
 * How long the sweep waits before it reads a checkout again that Polar reports `open` or
 * `confirmed`, or that it could not read.
 */
export const CHECKOUT_READ_INTERVAL_MS = 5 * 60 * 1000

/** How often the job of Polar moves the `open` checkouts after their expiry to `expired`. */
export const POLAR_CHECKOUT_EXPIRY_JOB_MS = 15 * 60 * 1000

/**
 * The time after the expiry of a checkout that the hold text names. The job of Polar expires the
 * checkout, the sweep reads it at its next read and removes the hold. The rest covers the run of
 * the sweep every minute, and a clock of Polar that runs behind the clock of the app.
 */
const CHECKOUT_HOLD_MARGIN_MS = POLAR_CHECKOUT_EXPIRY_JOB_MS + CHECKOUT_READ_INTERVAL_MS + 10 * 60 * 1000

/**
 * How long the sweep still checks a subscription after its end or after its past-due grace, or an
 * order after `ORDER_PAYMENT_GRACE_MS`.
 */
const SWEEP_WINDOW_MS = 24 * 60 * 60 * 1000

export type ManagedPlan = "free" | "paid" | "suspended"

const endsWithinMargin = (subscription: Subscription, now: Date): boolean =>
  subscription.cancelAtPeriodEnd &&
  subscription.currentPeriodEnd !== undefined &&
  now.getTime() >= subscription.currentPeriodEnd.getTime() - SETTLEMENT_MARGIN_MS

/**
 * When the past-due grace of the subscription ends: `pastDueGraceMs` after `past_due_at`, while
 * the subscription is `past_due`. A subscription in another status has no past-due grace.
 */
export const pastDueGraceEnd = (subscription: Subscription, pastDueGraceMs: number): Date | undefined =>
  subscription.status === "past_due" && subscription.pastDueAt !== undefined
    ? new Date(subscription.pastDueAt.getTime() + pastDueGraceMs)
    : undefined

const inPastDueGrace = (subscription: Subscription, now: Date, pastDueGraceMs: number): boolean => {
  const end = pastDueGraceEnd(subscription, pastDueGraceMs)
  return end !== undefined && now.getTime() < end.getTime()
}

/** Whether Polar collects the unpaid order: it is `pending`, and its subscription is billable. */
const polarCollects = (order: UnpaidOrder, subscriptions: ReadonlyArray<Subscription>): boolean =>
  order.status === "pending" &&
  subscriptions.some(
    (subscription) =>
      subscription.subscriptionId === order.subscriptionId && billableStatuses.includes(subscription.status)
  )

/**
 * The unpaid orders that a settlement can pay, oldest first: the ones that Polar no longer
 * collects, with paid overage of at least the `SETTLEMENT_MINIMUM` of their currency.
 */
const settleableOrders = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>
): ReadonlyArray<UnpaidOrder> =>
  unpaidOrders.filter(
    (order) => order.meteredTotal >= SETTLEMENT_MINIMUM[order.currency] && !polarCollects(order, subscriptions)
  )

/**
 * The unpaid orders with paid overage that the account owes: each one that Polar collects, and each
 * one that a settlement can pay. The app writes off each other order for good: Polar no longer
 * collects it, and a checkout cannot settle its paid overage. Other unpaid orders do not change
 * this, so a written-off order never comes back as a debt.
 */
export const owedOrders = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>
): ReadonlyArray<UnpaidOrder> =>
  unpaidOrders.filter(
    (order) =>
      order.meteredTotal > 0 &&
      (polarCollects(order, subscriptions) || order.meteredTotal >= SETTLEMENT_MINIMUM[order.currency])
  )

/**
 * What a settlement pays: the paid overage of each order, and the sum, in minor units of the
 * currency of the orders.
 */
export interface SettlementOffer {
  readonly currency: Currency
  readonly orders: ReadonlyArray<{ readonly orderId: string; readonly amount: number }>
  readonly amount: number
}

/**
 * The settlement that the account can pay: the paid overage of the oldest orders that a settlement
 * can pay, at most `SETTLEMENT_MAX_ORDERS` of them and at most the `SETTLEMENT_MAXIMUM` of their
 * currency together. A checkout has one currency, so the settlement pays only the orders in the
 * currency of the oldest order that fits. An order that does not fit, or has another currency,
 * waits for a later settlement. None when no such order exists, so that a settlement never pays
 * an order that Polar can still charge, or an order that the app wrote off.
 */
export const settlementFor = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>
): SettlementOffer | undefined => {
  let currency: Currency | undefined
  const orders: Array<{ readonly orderId: string; readonly amount: number }> = []
  let amount = 0
  for (const order of settleableOrders(subscriptions, unpaidOrders)) {
    if (
      order.currency === (currency ?? order.currency) &&
      orders.length < SETTLEMENT_MAX_ORDERS &&
      amount + order.meteredTotal <= SETTLEMENT_MAXIMUM[order.currency]
    ) {
      currency = order.currency
      orders.push({ orderId: order.orderId, amount: order.meteredTotal })
      amount += order.meteredTotal
    }
  }
  return currency === undefined ? undefined : { currency, orders, amount }
}

/** Whether the account did not pay for paid overage at `now` (see the rules above). */
export const owesOverage = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>,
  now: Date,
  pastDueGraceMs: number
): boolean =>
  owedOrders(subscriptions, unpaidOrders).some((order) => {
    const subscription = subscriptions.find((candidate) => candidate.subscriptionId === order.subscriptionId)
    if (subscription !== undefined && inPastDueGrace(subscription, now, pastDueGraceMs)) {
      return false
    }
    return (
      subscription?.pastDueAt !== undefined || now.getTime() - order.createdAt.getTime() >= ORDER_PAYMENT_GRACE_MS
    )
  })

/** The plan for the subscriptions and the unpaid orders of an account at `now`. */
export const planFor = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>,
  now: Date,
  pastDueGraceMs: number
): ManagedPlan => {
  if (owesOverage(subscriptions, unpaidOrders, now, pastDueGraceMs)) {
    return "suspended"
  }
  return subscriptions.some(
    (subscription) =>
      (subscription.status === "active" || inPastDueGrace(subscription, now, pastDueGraceMs)) &&
      !endsWithinMargin(subscription, now)
  )
    ? "paid"
    : "free"
}

/** Why an account needs a deletion hold. */
export type HoldReason =
  /** A billable subscription is not canceled at the period end. */
  | "subscription"
  /** A billable subscription is canceled at the period end, and runs to the end of the period. */
  | "canceledSubscription"
  /**
   * The usage push recorded paid overage that no order bills yet, and a billable subscription
   * bills it on its next order.
   */
  | "unbilledUsage"
  /**
   * The usage push recorded paid overage that no order bills yet, and no subscription is billable:
   * the last order of an ended subscription has not come, or the push has not sent or held an
   * event yet. Each clears within minutes. If it does not, an operator checks the account in Polar.
   */
  | "unsettledUsage"
  /** The account owes the paid overage of an unpaid order (`owedOrders`). */
  | "unpaidOverage"
  /** A checkout of the paid product is `open`: Polar can still take its payment. */
  | "openCheckout"
  /**
   * Polar took the payment of a checkout (`confirmed` or `succeeded`), and the app has not
   * recorded the subscription that it started.
   */
  | "paidCheckout"

/**
 * The reasons for a deletion hold on the account; none when the account has no billable
 * subscription, owes no paid overage and has no unfinished checkout. `unbilledUsage` is
 * `Subscriptions.unbilledUsage`, which does not count the overage that Polar drops when it ends a
 * subscription before its period end. `checkouts` are `Subscriptions.unfinishedCheckouts`.
 *
 * A billable subscription holds the account to the end of the period, also after a cancellation.
 * Paid overage needs the paid plan, and the paid plan needs a billable subscription, so the
 * account has a hold for as long as it can run up paid overage. The owner turns paid overage on
 * and off in the registry, which the app does not see, so a hold that followed the switch would
 * come too late. The cleanup of deleted accounts checks the same reasons before it deletes a
 * Polar customer, so it ends a paid period early only after an administrator removed the hold, and
 * only to stop a subscription that would charge again (see `Cleanup.ts`).
 */
export const holdFor = (
  subscriptions: ReadonlyArray<Subscription>,
  unpaidOrders: ReadonlyArray<UnpaidOrder>,
  unbilledUsage: boolean,
  checkouts: ReadonlyArray<Checkout>
): ReadonlyArray<HoldReason> => {
  const reasons: Array<HoldReason> = []
  const billable = subscriptions.filter((subscription) => billableStatuses.includes(subscription.status))
  if (billable.some((subscription) => !subscription.cancelAtPeriodEnd)) {
    reasons.push("subscription")
  } else if (billable.length > 0) {
    reasons.push("canceledSubscription")
  }
  if (unbilledUsage) {
    reasons.push(billable.length > 0 ? "unbilledUsage" : "unsettledUsage")
  }
  if (owedOrders(subscriptions, unpaidOrders).length > 0) {
    reasons.push("unpaidOverage")
  }
  if (checkouts.some((checkout) => checkout.status === "open")) {
    reasons.push("openCheckout")
  }
  if (checkouts.some((checkout) => checkout.status === "confirmed" || checkout.status === "succeeded")) {
    reasons.push("paidCheckout")
  }
  return reasons
}

const utcDateTime = new Intl.DateTimeFormat("en", { dateStyle: "long", timeStyle: "short", timeZone: "UTC" })

/** Where the person asks for help when a deletion hold stays and an operator has to act. */
export const CONTACT_EMAIL = "sales@golem.cloud"

/**
 * The reason text of the deletion hold. The registry shows it to the person who tries to delete
 * the account, so each sentence says what to do. `billingUrl` is the page of the billing app,
 * which shows how to pay an unpaid order: the customer portal while Polar collects it, else a
 * settlement.
 * Only a billable subscription has an invoice to come, so the text of `unsettledUsage` promises
 * none. The text of `openCheckout` names when the hold of the open checkout that expires last
 * ends; `checkouts` are the checkouts that gave the reasons. The hold ends only when Polar reports
 * the checkout `expired`, so after that time at `now` the text names a contact instead.
 */
export const holdReasonText = (
  reasons: ReadonlyArray<HoldReason>,
  billingUrl: string,
  checkouts: ReadonlyArray<Checkout>,
  now: Date
): string => {
  const openUntil =
    Math.max(
      ...checkouts.filter((checkout) => checkout.status === "open").map((checkout) => checkout.expiresAt.getTime())
    ) + CHECKOUT_HOLD_MARGIN_MS
  const sentences: Record<HoldReason, () => string> = {
    subscription: () =>
      `The account has a subscription to Pro. Cancel it in Manage billing at ${billingUrl}. The subscription then runs to the end of the period. Delete the account after the period ends.`,
    canceledSubscription: () =>
      "The subscription to Pro is canceled and runs to the end of the period. Delete the account after the period ends.",
    unbilledUsage: () =>
      "The account has paid overage that Polar has not billed yet. Polar bills it on the invoice at the end of the period. Delete the account after that invoice is paid.",
    unsettledUsage: () =>
      `The billing of the paid overage of the account is not final yet. Try again in an hour. If the delete still fails, contact Golem Cloud at ${CONTACT_EMAIL}.`,
    unpaidOverage: () => `An invoice for paid overage is not paid. Pay it on the billing page at ${billingUrl}.`,
    openCheckout: () =>
      openUntil > now.getTime()
        ? `A checkout for Pro from the billing page is open. Delete the account after ${utcDateTime.format(openUntil)} UTC.`
        : `A checkout for Pro from the billing page is past its expiry, and Polar has not closed it yet. Try again in an hour. If the delete still fails, contact Golem Cloud at ${CONTACT_EMAIL}.`,
    paidCheckout: () =>
      `Polar has a payment for Pro, and Golem Cloud billing has not recorded its subscription yet. Try again in an hour. If the delete still fails, contact Golem Cloud at ${CONTACT_EMAIL}.`
  }
  return ["Golem Cloud billing:", ...reasons.map((reason) => sentences[reason]())].join(" ")
}

export interface SettlementShape {
  /**
   * Sets the deletion hold of the account and moves the account to the plan of its subscriptions
   * at `now`. An account on the default plan (`BillingConfig.defaultPlanId`) moves only to the paid
   * plan. An account on a plan that the app does not manage keeps it, and gets the deletion hold
   * all the same. A deleted account keeps its plan and its deletion hold. The caller holds the
   * account lock of `Subscriptions`. Returns the reasons of the hold that the account has now, or
   * `deleted` when the registry deleted the account.
   */
  readonly reconcile: (
    accountId: string,
    now: Date
  ) => Effect.Effect<ReadonlyArray<HoldReason> | "deleted", RegistryError | SubscriptionsError | RegistryAccountsError>
  /**
   * Reconciles every account whose plan or deletion hold can change with the time alone: a
   * subscription that ends within the settlement margin, an order that is still pending after
   * `ORDER_PAYMENT_GRACE_MS`, a subscription whose past-due grace ends, and the candidates of
   * `Subscriptions.holdCandidates`. Before that, it reads from Polar each checkout that
   * `Subscriptions.checkoutsToRead` gives for `CHECKOUT_READ_DELAY_MS` before `now`, and
   * reconciles its account too.
   */
  readonly sweep: (now: Date) => Effect.Effect<void>
}

export const make: Effect.Effect<
  SettlementShape,
  never,
  BillingConfig | Polar | Registry | RegistryAccounts | Subscriptions
> = Effect.gen(
  function* () {
    const config = yield* BillingConfig
    const polar = yield* Polar
    const registry = yield* Registry
    const registryAccounts = yield* RegistryAccounts
    const subscriptions = yield* Subscriptions
    const planIds: Readonly<Record<ManagedPlan, string>> = config.plans
    const managed = new Set(Object.values(planIds))
    const billingUrl = config.baseUrl.origin

    // The billing account is the holder of every deletion hold of the app. The first successful
    // read is kept; a failed read is tried again at the next call.
    const holder = yield* Ref.make(Option.none<string>())
    const holderAccountId = Effect.flatMap(Ref.get(holder), (known) =>
      Option.isSome(known)
        ? Effect.succeed(known.value)
        : registry.currentToken(config.registryBillingToken).pipe(
            Effect.map((token) => token.accountId),
            Effect.tap((accountId) => Ref.set(holder, Option.some(accountId)))
          )
    )

    // The registry answers 404 both for a deleted account and for an account that the billing
    // account cannot see. Only a deleted account is not an error.
    const getAccount = (accountId: string) =>
      registry.getAccount(config.registryBillingToken, accountId).pipe(
        Effect.map(Option.some),
        Effect.catchIf(
          (error) => error.status === 404,
          (error) =>
            Effect.flatMap(registryAccounts.isDeleted(accountId), (deleted) =>
              deleted ? Effect.succeed(Option.none()) : Effect.fail(error)
            )
        )
      )

    const changePlan = (account: Account, plan: ManagedPlan) =>
      Effect.gen(function* () {
        const target = planIds[plan]
        if (account.planId === target) {
          return
        }
        if (account.planId === config.defaultPlanId) {
          // The account keeps the default plan until a subscription makes it paid. A trial, or a
          // subscription that ends before it is active, does not move it to the free plan.
          if (plan !== "paid") {
            return
          }
        } else if (!managed.has(account.planId)) {
          yield* Effect.logWarning("The account has a plan that subscriptions do not change").pipe(
            Effect.annotateLogs({ golemAccountId: account.id, planId: account.planId })
          )
          return
        }
        yield* registry.setAccountPlan(config.registryBillingToken, account, target)
        yield* Effect.logInfo("Changed the plan of the account").pipe(
          Effect.annotateLogs({ golemAccountId: account.id, fromPlanId: account.planId, toPlanId: target, plan })
        )
      })

    const reconcile = (accountId: string, now: Date) =>
      Effect.gen(function* () {
        const accountSubscriptions = yield* subscriptions.ofAccount(accountId, config.polar.productId)
        const unpaidOrders = yield* subscriptions.unpaidOrders(accountId, config.polar.productId)
        const checkouts = yield* subscriptions.unfinishedCheckouts(accountId)
        const found = yield* getAccount(accountId)
        if (Option.isNone(found)) {
          yield* Effect.logInfo("The registry deleted the account; its plan and deletion hold stay").pipe(
            Effect.annotateLogs({ golemAccountId: accountId })
          )
          return "deleted" as const
        }
        const account = found.value

        const reasons = holdFor(
          accountSubscriptions,
          unpaidOrders,
          yield* subscriptions.unbilledUsage(accountId),
          checkouts
        )
        const holderId = yield* holderAccountId
        // The hold goes on before the plan changes and comes off after it, so the account is never
        // on the paid plan without a hold.
        if (reasons.length > 0) {
          const reason = holdReasonText(reasons, billingUrl, checkouts, now)
          yield* registry.placeDeletionHold(config.registryBillingToken, accountId, holderId, reason)
          yield* subscriptions.recordHold(accountId, reason)
        }
        yield* changePlan(account, planFor(accountSubscriptions, unpaidOrders, now, config.pastDueGraceMs))
        if (reasons.length === 0) {
          yield* registry.removeDeletionHold(config.registryBillingToken, accountId, holderId)
          yield* subscriptions.forgetHold(accountId)
        }
        yield* Effect.logDebug("Set the deletion hold of the account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, reasons: reasons.join(",") })
        )
        return reasons
      }).pipe(
        // 409: the account changed between the read and the write; read it again.
        Effect.retry({
          times: 3,
          while: (error: RegistryError | SubscriptionsError | RegistryAccountsError) =>
            "status" in error && error.status === 409
        })
      )

    const settle = (accountId: string, now: Date) =>
      subscriptions.withAccountLock(accountId, reconcile(accountId, now)).pipe(
        Effect.catch((error) =>
          Effect.logError("Cannot settle the plan of the account").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, error: error.message })
          )
        )
      )

    // Only a final status from Polar removes the hold of the checkout. Polar answers the read from a
    // replica that can lag, so an `open` checkout after its expiry can be one that the person paid
    // just before it; the sweep reads it again until the job of Polar expires it. The sweep reads
    // and records under the lock of the account, as the checkout route does, so a click never
    // decides on an older status than the app recorded. A failed read records only the time of
    // the read, so the next read waits `CHECKOUT_READ_INTERVAL_MS`.
    const readCheckout = (checkout: Checkout, now: Date) =>
      subscriptions.withAccountLock(
        checkout.accountId,
        polar.getCheckout(checkout.checkoutId).pipe(
          Effect.map((session): CheckoutStatus | undefined => session.status),
          Effect.tap((status) =>
            Effect.logInfo("Read a checkout after its expiry").pipe(
              Effect.annotateLogs({ golemAccountId: checkout.accountId, checkoutId: checkout.checkoutId, status })
            )
          ),
          Effect.catch((error) =>
            Effect.logError("Cannot read the checkout from Polar").pipe(
              Effect.annotateLogs({
                golemAccountId: checkout.accountId,
                checkoutId: checkout.checkoutId,
                error: error.message
              }),
              Effect.as(undefined)
            )
          ),
          Effect.flatMap((status) => subscriptions.recordCheckoutStatus(checkout.checkoutId, status, now))
        )
      ).pipe(
        Effect.catch((error) =>
          Effect.logError("Cannot record the status of the checkout").pipe(
            Effect.annotateLogs({ checkoutId: checkout.checkoutId, error: error.message })
          )
        )
      )

    const sweep = (now: Date) =>
      Effect.gen(function* () {
        const at = (offset: number) => new Date(now.getTime() + offset)
        const expired = yield* subscriptions.checkoutsToRead(
          at(-CHECKOUT_READ_DELAY_MS),
          at(-CHECKOUT_READ_INTERVAL_MS)
        )
        yield* Effect.forEach(expired, (checkout) => readCheckout(checkout, now), { discard: true })
        const ending = yield* subscriptions.ending(
          config.polar.productId,
          at(-SWEEP_WINDOW_MS),
          at(SETTLEMENT_MARGIN_MS)
        )
        const awaitingPayment = yield* subscriptions.awaitingPayment(
          config.polar.productId,
          at(-ORDER_PAYMENT_GRACE_MS - SWEEP_WINDOW_MS),
          at(-ORDER_PAYMENT_GRACE_MS)
        )
        const pastDueGraceEnding = yield* subscriptions.pastDue(
          config.polar.productId,
          at(-config.pastDueGraceMs - SWEEP_WINDOW_MS),
          at(-config.pastDueGraceMs)
        )
        const holdCandidates = yield* subscriptions.holdCandidates(
          config.polar.productId,
          at(-SWEEP_WINDOW_MS),
          at(-HOLD_RECHECK_MS)
        )
        const candidates = [
          ...new Set([
            ...expired.map((checkout) => checkout.accountId),
            ...ending,
            ...awaitingPayment,
            ...pastDueGraceEnding,
            ...holdCandidates
          ])
        ]
        // The reconcile changes nothing for a deleted account, so the sweep skips it.
        const deleted = new Set(yield* registryAccounts.deletedAmong(candidates))
        yield* Effect.forEach(
          candidates.filter((accountId) => !deleted.has(accountId)),
          (accountId) => settle(accountId, now),
          { discard: true }
        )
      }).pipe(
        Effect.catch((error) =>
          Effect.logError("Cannot read the subscriptions and orders to settle").pipe(
            Effect.annotateLogs({ error: error.message })
          )
        )
      )

    return { reconcile, sweep }
  }
)

/** Runs the sweep every minute while the web app runs. Every replica runs it; the account lock orders them. */
export const sweeper: Layer.Layer<
  never,
  never,
  BillingConfig | Polar | Registry | RegistryAccounts | Subscriptions
> = Layer.effectDiscard(
  Effect.gen(function* () {
    const { sweep } = yield* make
    yield* Effect.forkScoped(
      Effect.repeat(
        Effect.suspend(() => sweep(new Date())),
        Schedule.spaced("1 minute")
      )
    )
  })
)
