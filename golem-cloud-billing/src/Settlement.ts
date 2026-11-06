import { Effect, Layer, Option, Ref, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { Registry, type RegistryError } from "./Registry.ts"
import { RegistryAccounts, type RegistryAccountsError, type UsageMode } from "./RegistryAccounts.ts"
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
 *   price is `pending`, and a payment of its subscription failed (`past_due_at`, which stays after
 *   Polar revokes the subscription) or the order is older than `ORDER_PAYMENT_GRACE_MS`. The plan has no monthly amounts and no overage, so the agents of the
 *   account stop until the order is paid. A new subscription does not end the suspension.
 * - `paid`: a subscription is `active` and does not end within `SETTLEMENT_MARGIN_MS`. Only this
 *   plan has paid overage.
 * - `free`: otherwise. A declined payment of the price alone, a cancellation, a pause and a trial
 *   give the free plan. A subscription that is canceled at the period end moves the account to
 *   free `SETTLEMENT_MARGIN_MS` before the end, so the usage push sends the last paid overage
 *   while Polar still bills it.
 *
 * The registry sets an account on a plan without overage to hardLimit. Only the owner can turn
 * paid overage on again, also after the account is back on the paid plan.
 *
 * Settlement also places a deletion hold on the account while the account can owe paid overage
 * (`holdFor`), so the registry refuses to delete it. Golem does not charge early: the person
 * deletes the account after the overage is billed and paid.
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

/** How long the sweep still checks a subscription after its end, or an order after the grace. */
const SWEEP_WINDOW_MS = 24 * 60 * 60 * 1000

export type ManagedPlan = "free" | "paid" | "suspended"

const endsWithinMargin = (subscription: Subscription, now: Date): boolean =>
  subscription.cancelAtPeriodEnd &&
  subscription.currentPeriodEnd !== undefined &&
  now.getTime() >= subscription.currentPeriodEnd.getTime() - SETTLEMENT_MARGIN_MS

/** Whether the account did not pay for paid overage at `now` (see the rules above). */
export const owesOverage = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  now: Date
): boolean =>
  pendingOrders.some((order) => {
    if (order.meteredAmount <= 0) {
      return false
    }
    const subscription = subscriptions.find((candidate) => candidate.subscriptionId === order.subscriptionId)
    return (
      subscription?.pastDueAt !== undefined || now.getTime() - order.createdAt.getTime() >= ORDER_PAYMENT_GRACE_MS
    )
  })

/** The plan for the subscriptions and the pending orders of an account at `now`. */
export const planFor = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  now: Date
): ManagedPlan => {
  if (owesOverage(subscriptions, pendingOrders, now)) {
    return "suspended"
  }
  return subscriptions.some((subscription) => subscription.status === "active" && !endsWithinMargin(subscription, now))
    ? "paid"
    : "free"
}

/** Why an account needs a deletion hold. */
export type HoldReason =
  /** A billable subscription is not canceled at the period end. */
  | "subscription"
  /** A billable subscription exists and paid overage is on, so the account can still use it. */
  | "overageOn"
  /**
   * A billable subscription exists and paid overage was on less than `SETTLEMENT_MARGIN_MS` ago,
   * so the usage push may not have recorded the last overage yet.
   */
  | "overageRecentlyOn"
  /** The usage push recorded paid overage that no order bills yet. */
  | "unbilledUsage"
  /** A pending order has an amount for a metered price. */
  | "unpaidOverage"

/**
 * The reasons for a deletion hold on the account at `now`; none when the account owes nothing and
 * can no longer run up paid overage. `unbilledUsage` is `Subscriptions.unbilledUsage`.
 */
export const holdFor = (
  subscriptions: ReadonlyArray<Subscription>,
  pendingOrders: ReadonlyArray<PendingOrder>,
  usageMode: UsageMode,
  unbilledUsage: boolean,
  now: Date
): ReadonlyArray<HoldReason> => {
  const reasons: Array<HoldReason> = []
  const billable = subscriptions.filter((subscription) => billableStatuses.includes(subscription.status))
  if (billable.some((subscription) => !subscription.cancelAtPeriodEnd)) {
    reasons.push("subscription")
  }
  if (billable.length > 0) {
    if (usageMode.mode === "allowOverage") {
      reasons.push("overageOn")
    } else if (
      usageMode.changedAt !== undefined &&
      now.getTime() - usageMode.changedAt.getTime() < SETTLEMENT_MARGIN_MS
    ) {
      reasons.push("overageRecentlyOn")
    }
  }
  if (unbilledUsage) {
    reasons.push("unbilledUsage")
  }
  if (pendingOrders.some((order) => order.meteredAmount > 0)) {
    reasons.push("unpaidOverage")
  }
  return reasons
}

const marginMinutes = SETTLEMENT_MARGIN_MS / (60 * 1000)

/**
 * The reason text of the deletion hold. The registry shows it to the person who tries to delete
 * the account, so each sentence says what to do. `billingUrl` is the page of the billing app.
 */
export const holdReasonText = (reasons: ReadonlyArray<HoldReason>, billingUrl: string): string => {
  const sentences: Record<HoldReason, string> = {
    subscription: `The account has a subscription to Paid. Cancel it in Manage billing at ${billingUrl}. The subscription then runs to the end of the period.`,
    overageOn:
      "Paid overage is on, and the account can use it until the subscription ends. Turn paid overage off, or wait until the subscription ends.",
    overageRecentlyOn: `Paid overage was on less than ${marginMinutes} minutes ago, and its last usage is not recorded yet. Try again in ${marginMinutes} minutes.`,
    unbilledUsage:
      "The account has paid overage that Polar has not billed yet. Polar bills it on the next invoice of the subscription. Delete the account after that invoice is paid.",
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
   * subscription that ends within the settlement margin, an order that is still pending after the
   * grace, and the candidates of `Subscriptions.holdCandidates`.
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

        const reasons = holdFor(
          accountSubscriptions,
          pendingOrders,
          yield* registryAccounts.usageMode(accountId),
          yield* subscriptions.unbilledUsage(accountId),
          now
        )
        const holderId = yield* holderAccountId
        if (reasons.length > 0) {
          yield* registry.placeDeletionHold(
            config.registryBillingToken,
            accountId,
            holderId,
            holdReasonText(reasons, billingUrl)
          )
        } else {
          yield* registry.removeDeletionHold(config.registryBillingToken, accountId, holderId)
        }
        yield* Effect.logDebug("Set the deletion hold of the account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, reasons: reasons.join(",") })
        )

        const plan = planFor(accountSubscriptions, pendingOrders, now)
        const target = planIds[plan]
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
        const holdCandidates = yield* subscriptions.holdCandidates(config.polar.productId, at(-SWEEP_WINDOW_MS))
        yield* Effect.forEach(
          new Set([...ending, ...awaitingPayment, ...holdCandidates]),
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
