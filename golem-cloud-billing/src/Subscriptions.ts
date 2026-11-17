import { Context, Data, Effect, Layer } from "effect"
import type pg from "pg"
import { Database, requireTables, type SchemaError } from "./Database.ts"

/**
 * The statuses in which Polar bills the usage events of a subscription (`billable` of a Polar
 * subscription). A subscription that is canceled at the period end stays `active` until the
 * period ends; a failed payment makes it `past_due`.
 */
export const billableStatuses: ReadonlyArray<string> = ["active", "trialing", "past_due"]

/** The state of a Polar subscription, as the last webhook for it reported it. */
export interface Subscription {
  readonly subscriptionId: string
  /** The Golem account ID: the external ID of the Polar customer. */
  readonly accountId: string
  readonly productId: string
  readonly status: string
  readonly currentPeriodEnd: Date | undefined
  readonly cancelAtPeriodEnd: boolean
  /**
   * When the subscription ended (`ended_at` of Polar). A cancellation ends it at or after
   * `currentPeriodEnd`. A revoke, in the dashboard or by Polar after the failed retries, ends it
   * before `currentPeriodEnd`, which Polar keeps.
   */
  readonly endedAt: Date | undefined
  /** `modified_at` of the subscription in Polar, or `created_at` before its first change. */
  readonly modifiedAt: Date
  /** The metered prices of the product: an order item of one of them bills paid overage. */
  readonly meteredPriceIds: ReadonlyArray<string>
  /**
   * When a payment of the subscription failed (`past_due_at` of Polar). Polar clears it when a
   * payment succeeds, and keeps it when it revokes the subscription after the failed retries.
   */
  readonly pastDueAt: Date | undefined
}

/** The state of a Polar order of a subscription, as the last webhook for it reported it. */
export interface Order {
  readonly orderId: string
  readonly accountId: string
  readonly subscriptionId: string
  /** `draft`, `pending` (not paid), `paid`, `refunded`, `partially_refunded` or `void`. */
  readonly status: string
  readonly createdAt: Date
  readonly modifiedAt: Date
  /**
   * `amount` is before the discount and the tax. `total` is what the order asks for the item: after
   * the discount, with the tax, and less the credit that a pending order uses
   * (`OrderTotals.itemTotals`).
   */
  readonly items: ReadonlyArray<{
    readonly productPriceId: string | null
    readonly amount: number
    readonly total: number
  }>
}

/**
 * An order that is not paid, with what its invoice charges for the items of metered prices, tax
 * included, in cents (`meteredTotal`): `pending`, or `void` and not settled. Polar voids the
 * pending orders of a subscription that it revokes, also after the failed retries of a payment. A
 * void collects nothing, so the account still owes the paid overage of the order until a
 * settlement pays it (see `Settlement.ts`).
 */
export interface UnpaidOrder {
  readonly orderId: string
  readonly subscriptionId: string
  /** `pending` or `void`. */
  readonly status: string
  readonly createdAt: Date
  readonly meteredTotal: number
}

/**
 * A payment of the paid overage of unpaid orders through an order of the settlement product (see
 * `Settlement.ts`): the order of the settlement, and the metered amount of each order that it
 * settles, in cents.
 */
export interface OverageSettlement {
  readonly accountId: string
  readonly settlementOrderId: string
  readonly orders: ReadonlyArray<{ readonly orderId: string; readonly amount: number }>
}

/**
 * The status of a Polar checkout. `confirmed`: the person sent the payment, and Polar processes
 * it. `succeeded`: Polar has the payment and creates the subscription.
 *
 * Polar moves a checkout from `open` to `confirmed` when the person pays, back to `open` when the
 * payment fails, and from `confirmed` to `succeeded`. A job of Polar moves each `open` checkout
 * after its expiry to `expired`. No status follows `succeeded`, `expired` or `failed` (which Polar
 * no longer sets), so these are final.
 */
export type CheckoutStatus = "open" | "expired" | "confirmed" | "succeeded" | "failed"

/**
 * A checkout of the paid product that the checkout route created for the account, as the app
 * last saw it.
 */
export interface Checkout {
  readonly checkoutId: string
  readonly accountId: string
  /**
   * The status of the checkout at the last read from Polar. The app records `open` when it creates
   * the checkout. While the status is `open` or `confirmed`, the checkout route reads the checkout
   * again at each click of the account, and the sweep reads it after `expiresAt`
   * (`Settlement.sweep`).
   */
  readonly status: CheckoutStatus
  /** When Polar stops taking the payment of an open checkout. */
  readonly expiresAt: Date
}

/** A failed query of the subscription tables. */
export class SubscriptionsError extends Data.TaggedError("SubscriptionsError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `The subscription tables failed: ${this.reason}`
  }
}

export interface SubscriptionsShape {
  /**
   * Stores the subscription, unless the stored state is newer. Polar can deliver webhooks out of
   * order, so an older state never replaces a newer one.
   */
  readonly record: (subscription: Subscription) => Effect.Effect<void, SubscriptionsError>
  /** Stores the order, unless the stored state is newer. */
  readonly recordOrder: (order: Order) => Effect.Effect<void, SubscriptionsError>
  /**
   * The subscriptions of the product that the account has, in every status, or of every product
   * when `productId` is undefined.
   */
  readonly ofAccount: (
    accountId: string,
    productId: string | undefined
  ) => Effect.Effect<ReadonlyArray<Subscription>, SubscriptionsError>
  /**
   * The unpaid orders of the subscriptions of the product that the account has, or of every
   * product when `productId` is undefined: `pending` or `void`, with no settlement.
   */
  readonly unpaidOrders: (
    accountId: string,
    productId: string | undefined
  ) => Effect.Effect<ReadonlyArray<UnpaidOrder>, SubscriptionsError>
  /**
   * Records that the settlement paid its orders. An order that has a settlement keeps it, so a
   * second delivery of the same payment changes nothing.
   */
  readonly recordSettlement: (settlement: OverageSettlement) => Effect.Effect<void, SubscriptionsError>
  /** The order of the settlement that paid the order, if one did. */
  readonly settlementOrderOf: (orderId: string) => Effect.Effect<string | undefined, SubscriptionsError>
  /**
   * Whether the usage push recorded paid overage of the account that an order can still bill: a
   * usage event that is not held, and is not sent yet, or was sent after the newest order of the
   * account while a subscription can still create an order (see `unbilledUsageEvents`). Polar
   * bills a sent event on the next order of the subscription. The push sends an unsent event at
   * its next run, or holds it when Polar would not bill it.
   */
  readonly unbilledUsage: (accountId: string) => Effect.Effect<boolean, SubscriptionsError>
  /**
   * The accounts whose deletion hold the sweep sets:
   *
   * - an account with a billable subscription of the product and no hold in `deletion_holds`:
   *   the subscription began before the app placed holds, or the last placement failed,
   * - an account without a billable subscription of the product, with usage events that the usage
   *   push recorded, sent or held after `since`. No webhook comes for these events: a new one can
   *   need a hold, and a held one no longer counts as unbilled, so it can remove the hold,
   * - an account with a usage settlement after `since`. An operator records it outside the app,
   *   and no webhook comes for it,
   * - an account with an unfinished checkout and no hold in `deletion_holds`: the placement of the
   *   checkout route failed,
   * - an account whose row in `deletion_holds` is older than `recheckBefore`. A hold that the app
   *   failed to remove keeps its row, so the sweep tries again, with no time limit.
   */
  readonly holdCandidates: (
    productId: string,
    since: Date,
    recheckBefore: Date
  ) => Effect.Effect<ReadonlyArray<string>, SubscriptionsError>
  /**
   * The accounts with a billable subscription of the product that is canceled at the period end,
   * with the period end between `from` and `to`.
   */
  readonly ending: (
    productId: string,
    from: Date,
    to: Date
  ) => Effect.Effect<ReadonlyArray<string>, SubscriptionsError>
  /** The accounts with a `pending` order of the product that Polar created between `from` and `to`. */
  readonly awaitingPayment: (
    productId: string,
    from: Date,
    to: Date
  ) => Effect.Effect<ReadonlyArray<string>, SubscriptionsError>
  /** The accounts with a `past_due` subscription of the product whose payment failed between `from` and `to`. */
  readonly pastDue: (
    productId: string,
    from: Date,
    to: Date
  ) => Effect.Effect<ReadonlyArray<string>, SubscriptionsError>
  /** The subscription of the product that changed last, for the plan page. */
  readonly latest: (
    accountId: string,
    productId: string
  ) => Effect.Effect<Subscription | undefined, SubscriptionsError>
  /** Records that the app placed a deletion hold on the account in the registry, with this reason. */
  readonly recordHold: (accountId: string, reason: string) => Effect.Effect<void, SubscriptionsError>
  /** Records that the app removed its deletion hold from the account. */
  readonly forgetHold: (accountId: string) => Effect.Effect<void, SubscriptionsError>
  /** Records a checkout that Polar created now, as `open`. */
  readonly recordCheckout: (checkout: {
    readonly checkoutId: string
    readonly accountId: string
    readonly expiresAt: Date
  }) => Effect.Effect<void, SubscriptionsError>
  /**
   * The unfinished checkouts of the account: Polar can still take their payment, or has taken it,
   * and the app has not recorded the subscription that they started. A checkout is finished when a
   * subscription webhook links it (`linkCheckout`), or when Polar reports it `expired` or `failed`.
   */
  readonly unfinishedCheckouts: (accountId: string) => Effect.Effect<ReadonlyArray<Checkout>, SubscriptionsError>
  /** Links the checkout of the account to the subscription that it started, which finishes it. */
  readonly linkCheckout: (
    checkoutId: string,
    accountId: string,
    subscriptionId: string
  ) => Effect.Effect<void, SubscriptionsError>
  /**
   * The unfinished checkouts that expired at or before `now` and are `open` or `confirmed`, and
   * that the app did not read from Polar after `readAfter`, of every account.
   */
  readonly checkoutsToRead: (now: Date, readAfter: Date) => Effect.Effect<ReadonlyArray<Checkout>, SubscriptionsError>
  /**
   * Records the status of the checkout that the app read from Polar at `readAt`. A final status
   * (`CheckoutStatus`) stays: Polar answers a read from a replica that can lag, so a later read
   * can report an older status.
   */
  readonly recordCheckoutStatus: (
    checkoutId: string,
    status: CheckoutStatus,
    readAt: Date
  ) => Effect.Effect<void, SubscriptionsError>
  /** Whether a webhook with this `webhook-id` was handled before. */
  readonly isDelivered: (webhookId: string) => Effect.Effect<boolean, SubscriptionsError>
  readonly markDelivered: (webhookId: string, eventType: string) => Effect.Effect<void, SubscriptionsError>
  /**
   * Runs `effect` while holding an advisory lock for the account, so that two webhooks of one
   * account, also on two replicas, change its plan one after the other.
   */
  readonly withAccountLock: <A, E, R>(
    accountId: string,
    effect: Effect.Effect<A, E, R>
  ) => Effect.Effect<A, E | SubscriptionsError, R>
}

export class Subscriptions extends Context.Service<Subscriptions, SubscriptionsShape>()(
  "golem-cloud-billing/Subscriptions"
) {}

/**
 * The tables of the app besides the tables of Better Auth: the subscriptions and their orders,
 * the checkouts of the paid product, the overage settlements that paid unpaid orders, the usage
 * events of the usage push, the deleted accounts that the cleanup has finished, the deletion holds
 * that the app placed in the registry, and the usage settlements of operators with their history.
 * `Migrations.migrate` creates them; the web app, the usage push and the cleanup only check that
 * they exist.
 *
 * An operator script outside the app records a usage settlement with the admin database user: it
 * sets `usage_settlements.settled_through` of the account, and adds a row with the same time, the
 * operator and the reason to `usage_settlement_history`. The app only reads `usage_settlements`.
 */
export const tables = `
  CREATE TABLE IF NOT EXISTS polar_subscriptions (
    subscription_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    product_id TEXT NOT NULL,
    status TEXT NOT NULL,
    current_period_end TIMESTAMPTZ,
    cancel_at_period_end BOOLEAN NOT NULL,
    modified_at TIMESTAMPTZ NOT NULL,
    metered_price_ids TEXT[] NOT NULL,
    past_due_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE INDEX IF NOT EXISTS polar_subscriptions_account ON polar_subscriptions (account_id, product_id);
  CREATE TABLE IF NOT EXISTS polar_orders (
    order_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    subscription_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    modified_at TIMESTAMPTZ NOT NULL,
    items JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE INDEX IF NOT EXISTS polar_orders_unpaid ON polar_orders (account_id, created_at)
    WHERE status IN ('pending', 'void');
  CREATE TABLE IF NOT EXISTS polar_checkouts (
    checkout_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    status TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    subscription_id TEXT,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE INDEX IF NOT EXISTS polar_checkouts_unfinished ON polar_checkouts (account_id)
    WHERE subscription_id IS NULL AND status IN ('open', 'confirmed', 'succeeded');
  CREATE TABLE IF NOT EXISTS overage_settlements (
    order_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    settlement_order_id TEXT NOT NULL,
    amount BIGINT NOT NULL,
    settled_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE TABLE IF NOT EXISTS polar_webhook_deliveries (
    webhook_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE TABLE IF NOT EXISTS usage_events (
    event_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    period TEXT NOT NULL,
    dimension TEXT NOT NULL,
    from_micro_units NUMERIC NOT NULL,
    to_micro_units NUMERIC NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    sent_at TIMESTAMPTZ,
    held_at TIMESTAMPTZ,
    CONSTRAINT usage_events_sent_or_held CHECK (sent_at IS NULL OR held_at IS NULL)
  );
  CREATE INDEX IF NOT EXISTS usage_events_unsent ON usage_events (created_at)
    WHERE sent_at IS NULL AND held_at IS NULL;
  CREATE INDEX IF NOT EXISTS usage_events_submitted ON usage_events (period, account_id, dimension);
  CREATE INDEX IF NOT EXISTS usage_events_account ON usage_events (account_id);
  CREATE TABLE IF NOT EXISTS deleted_account_cleanups (
    account_id UUID PRIMARY KEY,
    polar_customer_deleted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE TABLE IF NOT EXISTS deletion_holds (
    account_id UUID PRIMARY KEY,
    reason TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE TABLE IF NOT EXISTS usage_settlements (
    account_id UUID PRIMARY KEY,
    settled_through TIMESTAMPTZ NOT NULL
  );
  CREATE TABLE IF NOT EXISTS usage_settlement_history (
    settlement_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id UUID NOT NULL,
    settled_through TIMESTAMPTZ NOT NULL,
    operator TEXT NOT NULL,
    reason TEXT NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE INDEX IF NOT EXISTS usage_settlement_history_account ON usage_settlement_history (account_id, recorded_at)`

/** The tables that `tables` creates. */
export const tableNames: ReadonlyArray<string> = [
  "polar_subscriptions",
  "polar_orders",
  "polar_checkouts",
  "overage_settlements",
  "polar_webhook_deliveries",
  "usage_events",
  "deleted_account_cleanups",
  "deletion_holds",
  "usage_settlements",
  "usage_settlement_history"
]

/**
 * The usage events of account $1 that an order can still bill: not held, recorded after the usage
 * settlement of the account, and either
 *
 * - not sent yet. Polar bills only the events that it received, so an unsent event stays
 *   unbilled until the usage push sends it or holds it, also when an order is newer, or
 * - sent after the creation of the newest order of the account, while the latest subscription
 *   of the account can still create an order. The latest subscription is a subscription in a
 *   status of $2, which can always create an order, or else the subscription that ended last.
 *   An ended subscription cannot create an order when:
 *   - Polar ended it before its period end, for example with a revoke. Polar bills paid overage
 *     only on the order at the period end, and the product has no meter cycle, so Polar never
 *     bills the events that it received after the newest order. They are lost revenue.
 *   - Polar created an order of it at or after its period end: its last order. What that order
 *     billed is final. The app sets `sent_at` after Polar accepts an event, so an event that the
 *     last order billed can look newer than that order; the last order settles it.
 *
 * Whether an order is paid is a matter of `unpaidOrders`. When the app cannot tell that Polar has
 * billed or dropped an event, for example when the last order of a subscription never comes, an
 * operator checks the account in Polar and records a usage settlement outside the app: an event
 * that the push recorded up to `settled_through` no longer counts.
 */
const unbilledUsageEvents = `
  usage_events e
  WHERE e.account_id = $1 AND e.held_at IS NULL
    AND e.created_at > coalesce(
          (SELECT u.settled_through FROM usage_settlements u WHERE u.account_id = $1), '-infinity'::timestamptz)
    AND (
      e.sent_at IS NULL
      OR (
        e.sent_at > coalesce(
          (SELECT max(o.created_at) FROM polar_orders o WHERE o.account_id = $1), '-infinity'::timestamptz)
        AND NOT EXISTS (
          SELECT 1
            FROM (SELECT s.subscription_id, s.status, s.current_period_end, s.ended_at
                    FROM polar_subscriptions s
                   WHERE s.account_id = $1
                   ORDER BY (s.status = ANY($2)) DESC, s.ended_at DESC NULLS LAST, s.current_period_end DESC NULLS LAST
                   LIMIT 1) latest
           WHERE NOT (latest.status = ANY($2))
             AND (latest.ended_at < latest.current_period_end
                  OR EXISTS (
                    SELECT 1 FROM polar_orders o
                     WHERE o.subscription_id = latest.subscription_id AND o.created_at >= latest.current_period_end)))))`

interface SubscriptionRow {
  readonly subscription_id: string
  readonly account_id: string
  readonly product_id: string
  readonly status: string
  readonly current_period_end: Date | null
  readonly cancel_at_period_end: boolean
  readonly ended_at: Date | null
  readonly modified_at: Date
  readonly metered_price_ids: Array<string>
  readonly past_due_at: Date | null
}

const fromRow = (row: SubscriptionRow): Subscription => ({
  subscriptionId: row.subscription_id,
  accountId: row.account_id,
  productId: row.product_id,
  status: row.status,
  currentPeriodEnd: row.current_period_end ?? undefined,
  cancelAtPeriodEnd: row.cancel_at_period_end,
  endedAt: row.ended_at ?? undefined,
  modifiedAt: row.modified_at,
  meteredPriceIds: row.metered_price_ids,
  pastDueAt: row.past_due_at ?? undefined
})

const columns = `subscription_id, account_id::text AS account_id, product_id, status, current_period_end,
                cancel_at_period_end, ended_at, modified_at, metered_price_ids, past_due_at`

const failure = (error: unknown) =>
  new SubscriptionsError({ reason: error instanceof Error ? error.message : String(error) })

/** The condition of an unfinished checkout (`SubscriptionsShape.unfinishedCheckouts`) in `polar_checkouts`. */
const unfinished = (alias: string) =>
  `${alias}.subscription_id IS NULL AND ${alias}.status IN ('open', 'confirmed', 'succeeded')`

interface CheckoutRow {
  readonly checkout_id: string
  readonly account_id: string
  readonly status: CheckoutStatus
  readonly expires_at: Date
}

const checkoutColumns = "checkout_id, account_id::text AS account_id, status, expires_at"

const fromCheckoutRow = (row: CheckoutRow): Checkout => ({
  checkoutId: row.checkout_id,
  accountId: row.account_id,
  status: row.status,
  expiresAt: row.expires_at
})

/** Checks that the tables exist, and returns the queries. */
const make: Effect.Effect<SubscriptionsShape, SchemaError, Database> =
  Effect.gen(function* () {
    yield* requireTables(tableNames)
    const { pool } = yield* Database

    const query = <R extends object>(text: string, values: ReadonlyArray<unknown> = []) =>
      Effect.tryPromise({ try: () => pool.query<R & pg.QueryResultRow>(text, [...values]), catch: failure })

    const record = (subscription: Subscription) =>
      query(
        `INSERT INTO polar_subscriptions
           (subscription_id, account_id, product_id, status, current_period_end, cancel_at_period_end, ended_at,
            modified_at, metered_price_ids, past_due_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (subscription_id) DO UPDATE SET
           account_id = EXCLUDED.account_id,
           product_id = EXCLUDED.product_id,
           status = EXCLUDED.status,
           current_period_end = EXCLUDED.current_period_end,
           cancel_at_period_end = EXCLUDED.cancel_at_period_end,
           ended_at = EXCLUDED.ended_at,
           modified_at = EXCLUDED.modified_at,
           metered_price_ids = EXCLUDED.metered_price_ids,
           past_due_at = EXCLUDED.past_due_at,
           updated_at = clock_timestamp()
         WHERE polar_subscriptions.modified_at <= EXCLUDED.modified_at`,
        [
          subscription.subscriptionId,
          subscription.accountId,
          subscription.productId,
          subscription.status,
          subscription.currentPeriodEnd ?? null,
          subscription.cancelAtPeriodEnd,
          subscription.endedAt ?? null,
          subscription.modifiedAt,
          subscription.meteredPriceIds,
          subscription.pastDueAt ?? null
        ]
      ).pipe(Effect.asVoid)

    const recordOrder = (order: Order) =>
      query(
        `INSERT INTO polar_orders (order_id, account_id, subscription_id, status, created_at, modified_at, items)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (order_id) DO UPDATE SET
           status = EXCLUDED.status,
           modified_at = EXCLUDED.modified_at,
           items = EXCLUDED.items,
           updated_at = clock_timestamp()
         WHERE polar_orders.modified_at <= EXCLUDED.modified_at`,
        [
          order.orderId,
          order.accountId,
          order.subscriptionId,
          order.status,
          order.createdAt,
          order.modifiedAt,
          JSON.stringify(order.items)
        ]
      ).pipe(Effect.asVoid)

    const ofAccount = (accountId: string, productId: string | undefined) =>
      query<SubscriptionRow>(
        `SELECT ${columns} FROM polar_subscriptions WHERE account_id = $1 AND ($2::text IS NULL OR product_id = $2)`,
        [accountId, productId ?? null]
      ).pipe(Effect.map((result) => result.rows.map(fromRow)))

    // An order of a subscription that no webhook has reported yet has no metered prices to match;
    // the webhook of that subscription reconciles the plan again.
    const unpaidOrders = (accountId: string, productId: string | undefined) =>
      query<{ order_id: string; subscription_id: string; status: string; created_at: Date; metered_total: string }>(
        `SELECT o.order_id, o.subscription_id, o.status, o.created_at,
                coalesce((SELECT sum(i.total)
                            FROM jsonb_to_recordset(o.items) AS i("productPriceId" text, total bigint)
                           WHERE i."productPriceId" = ANY(s.metered_price_ids)), 0)::text AS metered_total
           FROM polar_orders o
           JOIN polar_subscriptions s ON s.subscription_id = o.subscription_id
          WHERE o.account_id = $1 AND ($2::text IS NULL OR s.product_id = $2) AND o.status IN ('pending', 'void')
            AND NOT EXISTS (SELECT 1 FROM overage_settlements x WHERE x.order_id = o.order_id)
          ORDER BY o.created_at, o.order_id`,
        [accountId, productId ?? null]
      ).pipe(
        Effect.map((result) =>
          result.rows.map((row) => ({
            orderId: row.order_id,
            subscriptionId: row.subscription_id,
            status: row.status,
            createdAt: row.created_at,
            meteredTotal: Number(row.metered_total)
          }))
        )
      )

    const recordSettlement = (settlement: OverageSettlement) =>
      query(
        `INSERT INTO overage_settlements (order_id, account_id, settlement_order_id, amount)
         SELECT o."orderId", $1, $2, o.amount
           FROM jsonb_to_recordset($3::jsonb) AS o("orderId" text, amount bigint)
         ON CONFLICT (order_id) DO NOTHING`,
        [settlement.accountId, settlement.settlementOrderId, JSON.stringify(settlement.orders)]
      ).pipe(Effect.asVoid)

    const settlementOrderOf = (orderId: string) =>
      query<{ settlement_order_id: string }>(
        "SELECT settlement_order_id FROM overage_settlements WHERE order_id = $1",
        [orderId]
      ).pipe(Effect.map((result) => result.rows[0]?.settlement_order_id))

    const unbilledUsage = (accountId: string) =>
      query<{ unbilled: boolean }>(`SELECT EXISTS (SELECT 1 FROM ${unbilledUsageEvents}) AS unbilled`, [
        accountId,
        billableStatuses
      ]).pipe(Effect.map((result) => result.rows[0]?.unbilled === true))

    const holdCandidates = (productId: string, since: Date, recheckBefore: Date) =>
      query<{ account_id: string }>(
        `SELECT s.account_id::text AS account_id
           FROM polar_subscriptions s
          WHERE s.product_id = $1 AND s.status = ANY($2)
            AND NOT EXISTS (SELECT 1 FROM deletion_holds h WHERE h.account_id = s.account_id)
         UNION
         SELECT e.account_id::text AS account_id
           FROM usage_events e
          WHERE greatest(e.created_at, e.sent_at, e.held_at) > $3
            AND NOT EXISTS (
              SELECT 1 FROM polar_subscriptions s
               WHERE s.account_id = e.account_id AND s.product_id = $1 AND s.status = ANY($2))
         UNION
         SELECT account_id::text AS account_id FROM usage_settlements WHERE settled_through > $3
         UNION
         SELECT c.account_id::text AS account_id
           FROM polar_checkouts c
          WHERE ${unfinished("c")}
            AND NOT EXISTS (SELECT 1 FROM deletion_holds h WHERE h.account_id = c.account_id)
         UNION
         SELECT account_id::text AS account_id FROM deletion_holds WHERE updated_at < $4`,
        [productId, billableStatuses, since, recheckBefore]
      ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    const recordHold = (accountId: string, reason: string) =>
      query(
        `INSERT INTO deletion_holds (account_id, reason) VALUES ($1, $2)
         ON CONFLICT (account_id) DO UPDATE SET reason = EXCLUDED.reason, updated_at = clock_timestamp()`,
        [accountId, reason]
      ).pipe(Effect.asVoid)

    const forgetHold = (accountId: string) =>
      query("DELETE FROM deletion_holds WHERE account_id = $1", [accountId]).pipe(Effect.asVoid)

    const ending = (productId: string, from: Date, to: Date) =>
      query<{ account_id: string }>(
        `SELECT DISTINCT account_id::text AS account_id
           FROM polar_subscriptions
          WHERE product_id = $1 AND status = ANY($2) AND cancel_at_period_end
            AND current_period_end BETWEEN $3 AND $4`,
        [productId, billableStatuses, from, to]
      ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    const awaitingPayment = (productId: string, from: Date, to: Date) =>
      query<{ account_id: string }>(
        `SELECT DISTINCT o.account_id::text AS account_id
           FROM polar_orders o
           JOIN polar_subscriptions s ON s.subscription_id = o.subscription_id
          WHERE s.product_id = $1 AND o.status = 'pending' AND o.created_at BETWEEN $2 AND $3`,
        [productId, from, to]
      ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    const pastDue = (productId: string, from: Date, to: Date) =>
      query<{ account_id: string }>(
        `SELECT DISTINCT account_id::text AS account_id
           FROM polar_subscriptions
          WHERE product_id = $1 AND status = 'past_due' AND past_due_at BETWEEN $2 AND $3`,
        [productId, from, to]
      ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    const latest = (accountId: string, productId: string) =>
      query<SubscriptionRow>(
        `SELECT ${columns}
           FROM polar_subscriptions
          WHERE account_id = $1 AND product_id = $2
          ORDER BY modified_at DESC
          LIMIT 1`,
        [accountId, productId]
      ).pipe(Effect.map((result) => (result.rows[0] === undefined ? undefined : fromRow(result.rows[0]))))

    const recordCheckout = (checkout: {
      readonly checkoutId: string
      readonly accountId: string
      readonly expiresAt: Date
    }) =>
      query("INSERT INTO polar_checkouts (checkout_id, account_id, status, expires_at) VALUES ($1, $2, 'open', $3)", [
        checkout.checkoutId,
        checkout.accountId,
        checkout.expiresAt
      ]).pipe(Effect.asVoid)

    const unfinishedCheckouts = (accountId: string) =>
      query<CheckoutRow>(
        `SELECT ${checkoutColumns} FROM polar_checkouts c WHERE c.account_id = $1 AND ${unfinished("c")}
          ORDER BY c.expires_at`,
        [accountId]
      ).pipe(Effect.map((result) => result.rows.map(fromCheckoutRow)))

    const linkCheckout = (checkoutId: string, accountId: string, subscriptionId: string) =>
      query(
        `UPDATE polar_checkouts SET subscription_id = $3
          WHERE checkout_id = $1 AND account_id = $2 AND subscription_id IS NULL`,
        [checkoutId, accountId, subscriptionId]
      ).pipe(Effect.asVoid)

    const checkoutsToRead = (now: Date, readAfter: Date) =>
      query<CheckoutRow>(
        `SELECT ${checkoutColumns} FROM polar_checkouts c
          WHERE ${unfinished("c")} AND c.status IN ('open', 'confirmed') AND c.expires_at <= $1
            AND (c.read_at IS NULL OR c.read_at <= $2)
          ORDER BY c.expires_at`,
        [now, readAfter]
      ).pipe(Effect.map((result) => result.rows.map(fromCheckoutRow)))

    const recordCheckoutStatus = (checkoutId: string, status: CheckoutStatus, readAt: Date) =>
      query(
        `UPDATE polar_checkouts SET status = $2, read_at = $3
          WHERE checkout_id = $1 AND status NOT IN ('succeeded', 'expired', 'failed')`,
        [checkoutId, status, readAt]
      ).pipe(Effect.asVoid)

    const isDelivered = (webhookId: string) =>
      query("SELECT 1 FROM polar_webhook_deliveries WHERE webhook_id = $1", [webhookId]).pipe(
        Effect.map((result) => (result.rowCount ?? 0) > 0)
      )

    const markDelivered = (webhookId: string, eventType: string) =>
      query(
        `INSERT INTO polar_webhook_deliveries (webhook_id, event_type) VALUES ($1, $2)
         ON CONFLICT (webhook_id) DO NOTHING`,
        [webhookId, eventType]
      ).pipe(Effect.asVoid)

    const withAccountLock = <A, E, R>(accountId: string, effect: Effect.Effect<A, E, R>) =>
      Effect.acquireUseRelease(
        Effect.tryPromise({
          try: async () => {
            const client = await pool.connect()
            try {
              await client.query("SELECT pg_advisory_lock(hashtextextended($1, 0))", [`plan:${accountId}`])
              return client
            } catch (error) {
              client.release()
              throw error
            }
          },
          catch: failure
        }),
        () => effect,
        (client) =>
          Effect.promise(async () => {
            try {
              await client.query("SELECT pg_advisory_unlock(hashtextextended($1, 0))", [`plan:${accountId}`])
              client.release()
            } catch (error) {
              // A connection that cannot unlock is destroyed, which also releases the lock.
              client.release(error instanceof Error ? error : true)
            }
          })
      )

    return {
      record,
      recordOrder,
      ofAccount,
      unpaidOrders,
      recordSettlement,
      settlementOrderOf,
      unbilledUsage,
      holdCandidates,
      recordHold,
      forgetHold,
      ending,
      awaitingPayment,
      pastDue,
      latest,
      recordCheckout,
      unfinishedCheckouts,
      linkCheckout,
      checkoutsToRead,
      recordCheckoutStatus,
      isDelivered,
      markDelivered,
      withAccountLock
    }
  })

/** The subscriptions of the web app and of the cleanup, in the schema that `Migrations.migrate` created. */
export const layer: Layer.Layer<Subscriptions, SchemaError, Database> = Layer.effect(Subscriptions, make)

