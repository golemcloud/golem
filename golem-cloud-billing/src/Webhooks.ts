import { webhooks } from "@polar-sh/sdk/2026-10"
import { Data, Effect, Redacted } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import type { RegistryError } from "./Registry.ts"
import * as Settlement from "./Settlement.ts"
import { type Subscription, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

type SubscriptionPayload = Extract<webhooks.WebhookPayload, { readonly type: `subscription.${string}` }>

const isSubscriptionPayload = (payload: webhooks.WebhookPayload): payload is SubscriptionPayload =>
  payload.type.startsWith("subscription.")

/** The HTTP response to a webhook delivery. Polar delivers again after any status but 2xx. */
export interface WebhookResponse {
  readonly status: number
  readonly body: string
}

/** A delivery that Polar must not send again: a bad signature or a body that is not JSON. */
class WebhookRejected extends Data.TaggedError("WebhookRejected")<{
  readonly status: number
  readonly reason: string
}> {}

/** A delivery of an event type that this version of the SDK does not know. */
class WebhookIgnored extends Data.TaggedError("WebhookIgnored")<{
  readonly eventType: string | null
}> {}

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

const toSubscription = (accountId: string, data: SubscriptionPayload["data"]): Subscription => ({
  subscriptionId: data.id,
  accountId,
  productId: data.product_id,
  status: data.status,
  currentPeriodEnd: new Date(data.current_period_end),
  cancelAtPeriodEnd: data.cancel_at_period_end,
  modifiedAt: new Date(data.modified_at ?? data.created_at)
})

/**
 * The handler of `POST /api/auth/polar/webhooks`.
 *
 * It verifies the Standard Webhooks signature, skips a `webhook-id` that it handled before, and
 * for a `subscription.*` event of the paid product stores the subscription and moves the account
 * to the plan of its subscriptions (`Settlement.planFor`). An account on any other plan keeps it.
 * A failed registry or database call answers 500, so Polar delivers the webhook again.
 */
export const makeHandler = Effect.gen(function* () {
  const config = yield* BillingConfig
  const subscriptions = yield* Subscriptions
  const settlement = yield* Settlement.make
  const secret = Redacted.value(config.polar.webhookSecret)

  const verify = (body: string, headers: Readonly<Record<string, string>>) =>
    Effect.tryPromise({
      try: () => webhooks.validateEvent(body, { ...headers }, secret),
      catch: (error) => {
        if (error instanceof webhooks.PolarWebhookUnknownTypeError) {
          return new WebhookIgnored({ eventType: typeof error.eventType === "string" ? error.eventType : null })
        }
        if (error instanceof webhooks.PolarWebhookVerificationError) {
          return new WebhookRejected({ status: 403, reason: error.message })
        }
        return new WebhookRejected({ status: 400, reason: "the body is not a webhook payload" })
      }
    })

  const handleSubscription = (
    payload: SubscriptionPayload
  ): Effect.Effect<void, RegistryError | SubscriptionsError> => {
    const data = payload.data
    const accountId = data.customer.external_id ?? undefined
    if (accountId === undefined || !uuid.test(accountId)) {
      return Effect.logWarning("A subscription of a Polar customer without a Golem account").pipe(
        Effect.annotateLogs({ subscriptionId: data.id, customerId: data.customer_id })
      )
    }
    if (data.product_id !== config.polar.productId) {
      return Effect.logInfo("A subscription of another product").pipe(
        Effect.annotateLogs({ subscriptionId: data.id, productId: data.product_id })
      )
    }
    return subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        yield* subscriptions.record(toSubscription(accountId, data))
        if (payload.type === "subscription.past_due") {
          yield* Effect.logWarning("A payment of the subscription failed").pipe(
            Effect.annotateLogs({ event: "payment_failed", golemAccountId: accountId, subscriptionId: data.id })
          )
        }
        yield* settlement.reconcile(accountId, new Date())
      })
    )
  }

  return (body: string, headers: Readonly<Record<string, string>>): Effect.Effect<WebhookResponse> =>
    Effect.gen(function* () {
      const payload = yield* verify(body, headers)
      // validateEvent has checked that the header is there.
      const webhookId = headers["webhook-id"] ?? ""
      if (yield* subscriptions.isDelivered(webhookId)) {
        return { status: 200, body: "already handled" }
      }
      if (isSubscriptionPayload(payload)) {
        yield* handleSubscription(payload)
      }
      yield* subscriptions.markDelivered(webhookId, payload.type)
      return { status: 200, body: "handled" }
    }).pipe(
      Effect.catch((error) => {
        if (error instanceof WebhookIgnored) {
          return Effect.logInfo("Ignored a webhook of an unknown event type").pipe(
            Effect.annotateLogs({ eventType: error.eventType ?? "none" }),
            Effect.as({ status: 200, body: "ignored" })
          )
        }
        if (error instanceof WebhookRejected) {
          return Effect.logWarning("Rejected a webhook").pipe(
            Effect.annotateLogs({ status: error.status, reason: error.reason }),
            Effect.as({ status: error.status, body: "rejected" })
          )
        }
        return Effect.logError("A webhook failed; Polar delivers it again").pipe(
          Effect.annotateLogs({ error: error.message }),
          Effect.as({ status: 500, body: "failed" })
        )
      })
    )
})
