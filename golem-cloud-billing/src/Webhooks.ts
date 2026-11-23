import { webhooks } from "@polar-sh/sdk/2026-10"
import { Data, Effect, Redacted } from "effect"
import { BillingConfig } from "./BillingConfig.ts"
import { type Currency, isCurrency } from "./Currency.ts"
import { itemTotals } from "./OrderTotals.ts"
import { SETTLED_ORDERS_METADATA_KEY } from "./Polar.ts"
import type { RegistryError } from "./Registry.ts"
import type { RegistryAccountsError } from "./RegistryAccounts.ts"
import * as Settlement from "./Settlement.ts"
import { type Order, type Subscription, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

type SubscriptionPayload = Extract<webhooks.WebhookPayload, { readonly type: `subscription.${string}` }>

const isSubscriptionPayload = (payload: webhooks.WebhookPayload): payload is SubscriptionPayload =>
  payload.type.startsWith("subscription.")

/** `order.created`, `order.updated` and `order.paid`; a refund does not change what the account owes. */
type OrderPayload = Extract<
  webhooks.WebhookPayload,
  { readonly type: "order.created" | "order.updated" | "order.paid" }
>

const isOrderPayload = (payload: webhooks.WebhookPayload): payload is OrderPayload =>
  payload.type === "order.created" || payload.type === "order.updated" || payload.type === "order.paid"

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

const isMetered = (price: { readonly amount_type: string }): boolean =>
  price.amount_type === "metered_unit" || price.amount_type === "metered_tiers"

// validateEvent checks the signature and the event type, not the shape of the data, so the
// optional parts of the payload are read with a fallback.
//
// The subscription lists its own prices, in its currency, and its product lists the prices of
// every currency. An order item of a metered price of either list bills paid overage.
const toSubscription = (accountId: string, currency: Currency, data: SubscriptionPayload["data"]): Subscription => ({
  subscriptionId: data.id,
  accountId,
  productId: data.product_id,
  currency,
  status: data.status,
  currentPeriodEnd: new Date(data.current_period_end),
  cancelAtPeriodEnd: data.cancel_at_period_end,
  endedAt: data.ended_at ? new Date(data.ended_at) : undefined,
  modifiedAt: data.modified_at ?? data.created_at,
  meteredPriceIds: [
    ...new Set([...(data.prices ?? []), ...(data.product?.prices ?? [])].filter(isMetered).map((price) => price.id))
  ],
  pastDueAt: data.past_due_at ? new Date(data.past_due_at) : undefined
})

/**
 * The part of an order that the person pays. Credit from the wallet of the customer, for example
 * after a downgrade, pays part of a new order, so its due amount is the total less that credit, and
 * the credit stays used while the order is pending. A void gives that credit back to the wallet, so
 * the person owes the total of a void order. Polar also takes credit that the customer gained after
 * the order against a void, and the order does not show it.
 */
const owedAmount = (data: OrderPayload["data"]): number =>
  data.status === "void" ? data.total_amount : data.due_amount

const toOrder = (accountId: string, subscriptionId: string, currency: Currency, data: OrderPayload["data"]): Order => {
  const items = data.items ?? []
  const totals = itemTotals({
    items: items.map((item) => ({ amount: item.amount, proration: item.proration === true })),
    discountAmount: data.discount_amount,
    amount: owedAmount(data)
  })
  return {
    orderId: data.id,
    accountId,
    subscriptionId,
    currency,
    status: data.status,
    createdAt: new Date(data.created_at),
    modifiedAt: data.modified_at ?? data.created_at,
    items: items.map((item, index) => ({
      productPriceId: item.product_price_id,
      amount: item.amount,
      total: totals[index] ?? 0
    }))
  }
}

/**
 * The handler of `POST /api/auth/polar/webhooks`.
 *
 * It verifies the Standard Webhooks signature, skips a `webhook-id` that it handled before, and
 * for a `subscription.*` or `order.*` event of the paid product stores the subscription or the
 * order, sets the deletion hold of the account (`Settlement.holdFor`) and moves the account to the
 * plan of its subscriptions and orders (`Settlement.planFor`). An account on any other plan keeps
 * it. A subscription links the checkout that started it (`checkout_id`), which finishes the
 * checkout. A paid order of the settlement product records a settlement of the orders in its
 * metadata. An event of a deleted account is stored and answers 200, so that the account does not
 * make Polar disable the endpoint. A subscription or an order in a currency that the app has no
 * prices for (`Currency.currencies`) is not stored, logs the error `unsupported_currency` and
 * answers 200: delivering it again changes nothing, and the Polar product must not have the
 * currency.
 * A failed registry or database call answers 500, so Polar delivers the webhook again. This
 * includes a 404 of the registry for an account that is not deleted: the permission share of the
 * billing account is missing.
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

  const unsupportedCurrency = (
    message: string,
    annotations: { readonly golemAccountId: string; readonly currency: string } & Record<string, string>
  ) => Effect.logError(message).pipe(Effect.annotateLogs({ event: "unsupported_currency", ...annotations }))

  const handleSubscription = (
    payload: SubscriptionPayload
  ): Effect.Effect<void, RegistryError | SubscriptionsError | RegistryAccountsError> => {
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
    const currency = data.currency
    if (!isCurrency(currency)) {
      return unsupportedCurrency("Ignored a subscription in a currency that the app has no prices for", {
        golemAccountId: accountId,
        subscriptionId: data.id,
        currency
      })
    }
    return subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        yield* subscriptions.record(toSubscription(accountId, currency, data))
        // The checkout that started the subscription no longer holds the account: the
        // subscription does, while it is billable.
        if (data.checkout_id) {
          yield* subscriptions.linkCheckout(data.checkout_id, accountId, data.id)
        }
        if (payload.type === "subscription.past_due") {
          yield* Effect.logWarning("A payment of the subscription failed").pipe(
            Effect.annotateLogs({ event: "payment_failed", golemAccountId: accountId, subscriptionId: data.id })
          )
        }
        yield* settlement.reconcile(accountId, new Date())
      })
    )
  }

  /**
   * A paid order of the settlement product settles each order in its metadata that the account
   * owes in the currency of the settlement, and reconciles the account. It settles nothing when its
   * price, the subtotal after the discount, is smaller than what the invoices of those orders charge
   * for paid overage. The price includes the tax, so the net amount is smaller than the price.
   * Polar delivers a paid order in several events (five in the sandbox), so an order that this
   * settlement already settled is skipped. An order that the account does not owe in that currency
   * logs a warning, for example an order that Polar charged after the checkout began: an operator
   * refunds that part of the settlement.
   */
  const handleSettlementOrder = (
    accountId: string,
    data: OrderPayload["data"]
  ): Effect.Effect<void, RegistryError | SubscriptionsError | RegistryAccountsError> => {
    if (data.status !== "paid") {
      return Effect.logInfo("A settlement order that is not paid").pipe(
        Effect.annotateLogs({ golemAccountId: accountId, settlementOrderId: data.id, status: data.status })
      )
    }
    const currency = data.currency
    if (!isCurrency(currency)) {
      return unsupportedCurrency("Ignored a settlement order in a currency that the app has no prices for", {
        golemAccountId: accountId,
        settlementOrderId: data.id,
        currency
      })
    }
    const listed = String(data.metadata?.[SETTLED_ORDERS_METADATA_KEY] ?? "")
      .split(",")
      .filter((orderId) => orderId !== "")
    return subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        const accountSubscriptions = yield* subscriptions.ofAccount(accountId, config.polar.productId)
        const unpaid = yield* subscriptions.unpaidOrders(accountId, config.polar.productId)
        const owed = Settlement.owedOrders(accountSubscriptions, unpaid).filter(
          (order) => listed.includes(order.orderId) && order.currency === currency
        )
        const notOwed: Array<string> = []
        for (const orderId of listed.filter((orderId) => !owed.some((order) => order.orderId === orderId))) {
          if ((yield* subscriptions.settlementOrderOf(orderId)) !== data.id) {
            notOwed.push(orderId)
          }
        }
        if (notOwed.length > 0) {
          yield* Effect.logWarning("A settlement pays orders that the account does not owe").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, settlementOrderId: data.id, orderIds: notOwed.join(",") })
          )
        }
        const amount = owed.reduce((sum, order) => sum + order.meteredTotal, 0)
        const paid =
          typeof data.subtotal_amount === "number" ? data.subtotal_amount - (data.discount_amount ?? 0) : 0
        if (paid < amount) {
          yield* Effect.logError("A settlement paid less than the paid overage of its orders").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, settlementOrderId: data.id, currency, paid, amount })
          )
          return
        }
        if (owed.length > 0) {
          yield* subscriptions.recordSettlement({
            accountId,
            settlementOrderId: data.id,
            currency,
            orders: owed.map((order) => ({ orderId: order.orderId, amount: order.meteredTotal }))
          })
          yield* Effect.logInfo("Recorded a settlement of paid overage").pipe(
            Effect.annotateLogs({
              golemAccountId: accountId,
              settlementOrderId: data.id,
              orderIds: owed.map((order) => order.orderId).join(","),
              currency,
              amount
            })
          )
        }
        yield* settlement.reconcile(accountId, new Date())
      })
    )
  }

  const handleOrder = (payload: OrderPayload): Effect.Effect<void, RegistryError | SubscriptionsError | RegistryAccountsError> => {
    const data = payload.data
    const accountId = data.customer?.external_id ?? undefined
    if (accountId === undefined || !uuid.test(accountId)) {
      return Effect.logWarning("An order of a Polar customer without a Golem account").pipe(
        Effect.annotateLogs({ orderId: data.id, customerId: data.customer_id })
      )
    }
    if (data.product_id === config.polar.settlementProductId) {
      return handleSettlementOrder(accountId, data)
    }
    if (data.product_id !== config.polar.productId || data.subscription_id === null) {
      return Effect.logInfo("An order that is not of a subscription of the paid product").pipe(
        Effect.annotateLogs({ orderId: data.id, productId: data.product_id ?? "none" })
      )
    }
    const subscriptionId = data.subscription_id
    const currency = data.currency
    if (!isCurrency(currency)) {
      return unsupportedCurrency("Ignored an order in a currency that the app has no prices for", {
        golemAccountId: accountId,
        orderId: data.id,
        currency
      })
    }
    return subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        yield* subscriptions.recordOrder(toOrder(accountId, subscriptionId, currency, data))
        if (data.status === "paid") {
          const settlementOrderId = yield* subscriptions.settlementOrderOf(data.id)
          if (settlementOrderId !== undefined) {
            yield* Effect.logError("An order was paid after its settlement; refund the settlement order").pipe(
              Effect.annotateLogs({ golemAccountId: accountId, orderId: data.id, settlementOrderId })
            )
          }
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
      } else if (isOrderPayload(payload)) {
        yield* handleOrder(payload)
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
