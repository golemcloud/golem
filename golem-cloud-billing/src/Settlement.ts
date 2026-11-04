import { Effect, Layer, Schedule } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { Registry, type RegistryError } from "./Registry.ts"
import { billableStatuses, type Subscription, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

/**
 * Settlement: the paid overage of an account reaches Polar while Polar can still bill it.
 *
 * Polar bills a usage event only when the customer has a billable subscription at the time that
 * Polar processes the event. It skips the events that arrive later. The usage push runs every 10
 * minutes and the executors report usage every minute, so the overage of the last minutes before
 * a subscription ends would arrive too late. Polar also does not bill usage during a trial, and
 * a past_due subscription can end without a payment.
 *
 * So the account has paid overage only while its subscription is `active` and does not end
 * within `SETTLEMENT_MARGIN_MS`. Otherwise a billable subscription gives the capped plan: the
 * monthly amounts of the paid plan without paid overage. The registry sets an account on a plan
 * without overage to hardLimit, and only the owner can turn paid overage on again.
 */

/**
 * How long before the scheduled end of a subscription the account moves to the capped plan. It
 * is longer than the interval of the usage push and the usage report interval of the executors
 * together, so the push sends the last paid overage before the subscription ends.
 */
export const SETTLEMENT_MARGIN_MS = 15 * 60 * 1000

/** How long after the end of a subscription the sweep still checks it, for a late webhook. */
const SWEEP_AFTER_END_MS = 24 * 60 * 60 * 1000

export type ManagedPlan = "free" | "paid" | "paidCapped"

const endsWithinMargin = (subscription: Subscription, now: Date): boolean =>
  subscription.cancelAtPeriodEnd &&
  subscription.currentPeriodEnd !== undefined &&
  now.getTime() >= subscription.currentPeriodEnd.getTime() - SETTLEMENT_MARGIN_MS

/**
 * The plan for the subscriptions of an account at `now`:
 *
 * - `paid` when a subscription is `active` and does not end within the settlement margin;
 * - `paidCapped` when a subscription is billable otherwise: `trialing`, `past_due`, or ending;
 * - `free` without a billable subscription.
 */
export const planFor = (subscriptions: ReadonlyArray<Subscription>, now: Date): ManagedPlan => {
  const billable = subscriptions.filter((subscription) => billableStatuses.includes(subscription.status))
  if (billable.some((subscription) => subscription.status === "active" && !endsWithinMargin(subscription, now))) {
    return "paid"
  }
  return billable.length > 0 ? "paidCapped" : "free"
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
