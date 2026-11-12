import { Effect, Layer, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { Registry, type RegistryError } from "./Registry.ts"
import { type PendingOrder, type Subscription, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

/**
 * Settlement: the plan of an account follows what the account has paid.
 *
 * The paid plan is prepaid with optional postpay: its price buys the monthly amounts in advance,
 * and Polar bills the paid overage after the period, on the order of the next renewal or on the
 * last order after a cancellation. Polar bills a usage event only if the customer has a billable
 * subscription when Polar processes the event, and it skips the events that arrive later. It
 * bills no usage during a trial.
 *
 * - `suspended`: the account did not pay for paid overage. An order with an amount for a metered
 *   price is `pending`, its subscription is not in the past-due grace, and a payment of the
 *   subscription failed (`past_due_at`, which stays after Polar revokes the subscription) or the
 *   order is older than `ORDER_PAYMENT_GRACE_MS`. The plan has no monthly amounts and no overage,
 *   so the agents of the account stop until the order is paid. A new subscription does not end the
 *   suspension.
 * - `paid`: a subscription is `active` and does not end within `SETTLEMENT_MARGIN_MS`, or a
 *   subscription is in the past-due grace. Only this plan has paid overage.
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
 * The registry sets an account on a plan without overage to hardLimit. Only the owner can turn
 * paid overage on again, also after the account is back on the paid plan.
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

/** Whether the account did not pay for paid overage at `now` (see the rules above). */
export const owesOverage = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  now: Date,
  pastDueGraceMs: number
): boolean =>
  pendingOrders.some((order) => {
    if (order.meteredAmount <= 0) {
      return false
    }
    const subscription = subscriptions.find((candidate) => candidate.subscriptionId === order.subscriptionId)
    if (subscription !== undefined && inPastDueGrace(subscription, now, pastDueGraceMs)) {
      return false
    }
    return (
      subscription?.pastDueAt !== undefined || now.getTime() - order.createdAt.getTime() >= ORDER_PAYMENT_GRACE_MS
    )
  })

/** The plan for the subscriptions and the pending orders of an account at `now`. */
export const planFor = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  now: Date,
  pastDueGraceMs: number
): ManagedPlan => {
  if (owesOverage(subscriptions, pendingOrders, now, pastDueGraceMs)) {
    return "suspended"
  }
  return subscriptions.some(
    (subscription) =>
      (subscription.status === "active" && !endsWithinMargin(subscription, now)) ||
      inPastDueGrace(subscription, now, pastDueGraceMs)
  )
    ? "paid"
    : "free"
}

export interface SettlementShape {
  /**
   * Moves the account to the plan of its subscriptions at `now`. An account on a plan that the
   * app does not manage keeps it. The caller holds the account lock of `Subscriptions`.
   */
  readonly reconcile: (accountId: string, now: Date) => Effect.Effect<void, RegistryError | SubscriptionsError>
  /**
   * Reconciles every account whose plan can change with the time alone: a subscription that ends
   * within the settlement margin, an order that is still pending after `ORDER_PAYMENT_GRACE_MS`,
   * and a subscription whose past-due grace ends.
   */
  readonly sweep: (now: Date) => Effect.Effect<void>
}

export const make: Effect.Effect<SettlementShape, never, BillingConfig | Registry | Subscriptions> = Effect.gen(
  function* () {
    const config = yield* BillingConfig
    const registry = yield* Registry
    const subscriptions = yield* Subscriptions
    const planIds: Readonly<Record<ManagedPlan, string>> = config.plans
    const managed = new Set(Object.values(planIds))

    const reconcile = (accountId: string, now: Date) =>
      Effect.gen(function* () {
        const plan = planFor(
          yield* subscriptions.ofAccount(accountId, config.polar.productId),
          yield* subscriptions.pendingOrders(accountId, config.polar.productId),
          now,
          config.pastDueGraceMs
        )
        const target = planIds[plan]
        const account = yield* registry.getAccount(config.registryBillingToken, accountId)
        if (account.planId === target) {
          return
        }
        if (!managed.has(account.planId)) {
          yield* Effect.logWarning("The account has a plan that subscriptions do not change").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, planId: account.planId })
          )
          return
        }
        yield* registry.setAccountPlan(config.registryBillingToken, account, target)
        yield* Effect.logInfo("Changed the plan of the account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, fromPlanId: account.planId, toPlanId: target, plan })
        )
      }).pipe(
        // 409: the account changed between the read and the write; read it again.
        Effect.retry({ times: 3, while: (error: RegistryError | SubscriptionsError) => "status" in error && error.status === 409 })
      )

    const settle = (accountId: string, now: Date) =>
      subscriptions.withAccountLock(accountId, reconcile(accountId, now)).pipe(
        Effect.catch((error) =>
          Effect.logError("Cannot settle the plan of the account").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, error: error.message })
          )
        )
      )

    const sweep = (now: Date) =>
      Effect.gen(function* () {
        const at = (offset: number) => new Date(now.getTime() + offset)
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
        yield* Effect.forEach(
          new Set([...ending, ...awaitingPayment, ...pastDueGraceEnding]),
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
export const sweeper: Layer.Layer<never, never, BillingConfig | Registry | Subscriptions> = Layer.effectDiscard(
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
