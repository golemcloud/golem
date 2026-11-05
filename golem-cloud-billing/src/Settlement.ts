import { Effect, Layer, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { Registry, type RegistryError } from "./Registry.ts"
import { type Subscription, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

/**
 * Settlement: the plan of an account follows what the account has paid.
 *
 * The paid plan is prepaid with optional postpay: its price buys the monthly amounts in advance,
 * and Polar bills the paid overage after the period. Polar bills a usage event only if the
 * customer has a billable subscription when Polar processes the event, and it skips the events
 * that arrive later. It bills no usage during a trial.
 *
 * - `paid`: a subscription is `active` and does not end within `SETTLEMENT_MARGIN_MS`. Only this
 *   plan has paid overage.
 * - `suspended`: the account owes a payment. A subscription is `past_due` or `unpaid`, or Polar
 *   revoked it while it was unpaid. The plan has no monthly amounts and no overage, so the agents
 *   of the account stop, and the account gets no free usage while it owes the payment.
 * - `free`: otherwise, also after a cancellation. A subscription that the customer canceled at the
 *   period end moves the account to free `SETTLEMENT_MARGIN_MS` before the end, so the usage push
 *   sends the last paid overage while Polar still bills it.
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

/** How long after the end of a subscription the sweep still checks it, for a late webhook. */
const SWEEP_AFTER_END_MS = 24 * 60 * 60 * 1000

export type ManagedPlan = "free" | "paid" | "suspended"

const endsWithinMargin = (subscription: Subscription, now: Date): boolean =>
  subscription.cancelAtPeriodEnd &&
  subscription.currentPeriodEnd !== undefined &&
  now.getTime() >= subscription.currentPeriodEnd.getTime() - SETTLEMENT_MARGIN_MS

const owesPayment = (subscription: Subscription): boolean =>
  subscription.status === "past_due" ||
  subscription.status === "unpaid" ||
  (subscription.status === "canceled" && subscription.pastDueAt !== undefined)

/** The plan for the subscriptions of an account at `now` (see the rules above). */
export const planFor = (subscriptions: ReadonlyArray<Subscription>, now: Date): ManagedPlan => {
  if (subscriptions.some((subscription) => subscription.status === "active" && !endsWithinMargin(subscription, now))) {
    return "paid"
  }
  return subscriptions.some(owesPayment) ? "suspended" : "free"
}

export interface SettlementShape {
  /**
   * Moves the account to the plan of its subscriptions at `now`. An account on a plan that the
   * app does not manage keeps it. The caller holds the account lock of `Subscriptions`.
   */
  readonly reconcile: (accountId: string, now: Date) => Effect.Effect<void, RegistryError | SubscriptionsError>
  /** Reconciles every account whose subscription ends within the settlement margin. */
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
        const plan = planFor(yield* subscriptions.ofAccount(accountId, config.polar.productId), now)
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

    const sweep = (now: Date) =>
      subscriptions
        .ending(
          config.polar.productId,
          new Date(now.getTime() - SWEEP_AFTER_END_MS),
          new Date(now.getTime() + SETTLEMENT_MARGIN_MS)
        )
        .pipe(
          Effect.flatMap((accounts) =>
            Effect.forEach(
              accounts,
              (accountId) =>
                subscriptions.withAccountLock(accountId, reconcile(accountId, now)).pipe(
                  Effect.catch((error) =>
                    Effect.logError("Cannot settle the plan of the account").pipe(
                      Effect.annotateLogs({ golemAccountId: accountId, error: error.message })
                    )
                  )
                ),
              { discard: true }
            )
          ),
          Effect.catch((error) =>
            Effect.logError("Cannot read the ending subscriptions").pipe(Effect.annotateLogs({ error: error.message }))
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
