import { Effect, Layer, Option, Ref, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { type Account, Registry, type RegistryError } from "./Registry.ts"
import { RegistryAccounts, type RegistryAccountsError } from "./RegistryAccounts.ts"
import {
  billableStatuses,
  type PendingOrder,
  type Subscription,
  Subscriptions,
  type SubscriptionsError
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
 *
 * Settlement also places a deletion hold on the account while the account has a billable
 * subscription or owes paid overage (`holdFor`), so the registry refuses to delete it. Golem does
 * not charge early and does not end a paid period early: the person deletes the account after the
 * subscription ends and Polar has billed the paid overage and been paid for it. When Polar ends a
 * subscription before its period end, it never bills the paid overage after the newest order, and
 * that overage does not hold the account.
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
 * How old a recorded deletion hold gets before the sweep reconciles the account again. A placement
 * refreshes the record, so a held account is checked about once in this time. A removal that
 * failed keeps the old record, so the sweep tries it again, long after the last webhook.
 */
export const HOLD_RECHECK_MS = 60 * 60 * 1000

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
  /** A pending order has an amount for a metered price. */
  | "unpaidOverage"

/**
 * The reasons for a deletion hold on the account; none when the account has no billable
 * subscription and owes no paid overage. `unbilledUsage` is `Subscriptions.unbilledUsage`, which
 * does not count the overage that Polar drops when it ends a subscription before its period end.
 *
 * A billable subscription holds the account to the end of the period, also after a cancellation.
 * Paid overage needs the paid plan, and the paid plan needs a billable subscription, so the
 * account has a hold for as long as it can run up paid overage. The owner turns paid overage on
 * and off in the registry, which the app does not see, so a hold that followed the switch would
 * come too late. The cleanup of deleted accounts checks the same reasons before it deletes a
 * Polar customer, so it never ends a paid period early.
 */
export const holdFor = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  unbilledUsage: boolean
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
  if (pendingOrders.some((order) => order.meteredAmount > 0)) {
    reasons.push("unpaidOverage")
  }
  return reasons
}

/** Where the person asks for help when a deletion hold stays and an operator has to act. */
export const SUPPORT_EMAIL = "contact@golem.cloud"

/**
 * The reason text of the deletion hold. The registry shows it to the person who tries to delete
 * the account, so each sentence says what to do. `billingUrl` is the page of the billing app.
 * Only a billable subscription has an invoice to come, so the text of `unsettledUsage` promises
 * none.
 */
export const holdReasonText = (reasons: ReadonlyArray<HoldReason>, billingUrl: string): string => {
  const sentences: Record<HoldReason, string> = {
    subscription: `The account has a subscription to Paid. Cancel it in Manage billing at ${billingUrl}. The subscription then runs to the end of the period. Delete the account after the period ends.`,
    canceledSubscription:
      "The subscription to Paid is canceled and runs to the end of the period. Delete the account after the period ends.",
    unbilledUsage:
      "The account has paid overage that Polar has not billed yet. Polar bills it on the invoice at the end of the period. Delete the account after that invoice is paid.",
    unsettledUsage: `The billing of the paid overage of the account is not final yet. Try again in an hour. If the delete still fails, contact support at ${SUPPORT_EMAIL}.`,
    unpaidOverage: `An invoice for paid overage is not paid. Pay it in Manage billing at ${billingUrl}.`
  }
  return ["Golem Cloud billing:", ...reasons.map((reason) => sentences[reason])].join(" ")
}

export interface SettlementShape {
  /**
   * Sets the deletion hold of the account and moves the account to the plan of its subscriptions
   * at `now`. An account on a plan that the app does not manage keeps it, and gets the deletion
   * hold all the same. A deleted account keeps its plan and its deletion hold. The caller holds
   * the account lock of `Subscriptions`.
   */
  readonly reconcile: (
    accountId: string,
    now: Date
  ) => Effect.Effect<void, RegistryError | SubscriptionsError | RegistryAccountsError>
  /**
   * Reconciles every account whose plan or deletion hold can change with the time alone: a
   * subscription that ends within the settlement margin, an order that is still pending after
   * `ORDER_PAYMENT_GRACE_MS`, a subscription whose past-due grace ends, and the candidates of
   * `Subscriptions.holdCandidates`.
   */
  readonly sweep: (now: Date) => Effect.Effect<void>
}

export const make: Effect.Effect<SettlementShape, never, BillingConfig | Registry | RegistryAccounts | Subscriptions> = Effect.gen(
  function* () {
    const config = yield* BillingConfig
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
        if (!managed.has(account.planId)) {
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
        const pendingOrders = yield* subscriptions.pendingOrders(accountId, config.polar.productId)
        const found = yield* getAccount(accountId)
        if (Option.isNone(found)) {
          yield* Effect.logInfo("The registry deleted the account; its plan and deletion hold stay").pipe(
            Effect.annotateLogs({ golemAccountId: accountId })
          )
          return
        }
        const account = found.value

        const reasons = holdFor(accountSubscriptions, pendingOrders, yield* subscriptions.unbilledUsage(accountId))
        const holderId = yield* holderAccountId
        // The hold goes on before the plan changes and comes off after it, so the account is never
        // on the paid plan without a hold.
        if (reasons.length > 0) {
          const reason = holdReasonText(reasons, billingUrl)
          yield* registry.placeDeletionHold(config.registryBillingToken, accountId, holderId, reason)
          yield* subscriptions.recordHold(accountId, reason)
        }
        yield* changePlan(account, planFor(accountSubscriptions, pendingOrders, now, config.pastDueGraceMs))
        if (reasons.length === 0) {
          yield* registry.removeDeletionHold(config.registryBillingToken, accountId, holderId)
          yield* subscriptions.forgetHold(accountId)
        }
        yield* Effect.logDebug("Set the deletion hold of the account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, reasons: reasons.join(",") })
        )
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
        const holdCandidates = yield* subscriptions.holdCandidates(
          config.polar.productId,
          at(-SWEEP_WINDOW_MS),
          at(-HOLD_RECHECK_MS)
        )
        const candidates = [...new Set([...ending, ...awaitingPayment, ...pastDueGraceEnding, ...holdCandidates])]
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
export const sweeper: Layer.Layer<never, never, BillingConfig | Registry | RegistryAccounts | Subscriptions> = Layer.effectDiscard(
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
