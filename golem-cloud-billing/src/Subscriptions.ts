import { Context, Data, Effect, Layer } from "effect"
import type pg from "pg"
import { Auth } from "./Auth.ts"
import { Database, withAdvisoryLock } from "./Database.ts"

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
  readonly items: ReadonlyArray<{ readonly productPriceId: string | null; readonly amount: number }>
}

/** An order that is not paid, with the amount of its items of metered prices, in cents. */
export interface PendingOrder {
  readonly orderId: string
  readonly subscriptionId: string
  readonly createdAt: Date
  readonly meteredAmount: number
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
   * The `pending` orders of the subscriptions of the product that the account has, or of every
   * product when `productId` is undefined.
   */
  readonly pendingOrders: (
    accountId: string,
    productId: string | undefined
  ) => Effect.Effect<ReadonlyArray<PendingOrder>, SubscriptionsError>
  /**
   * Whether the usage push recorded paid overage of the account that no order bills yet: a usage
   * event that is not held and is newer than the newest order of the account. Polar bills a sent
   * event on the next order of the subscription, and the push sends an unsent event at its next run.
   */
  readonly unbilledUsage: (accountId: string) => Effect.Effect<boolean, SubscriptionsError>
  /**
   * The accounts whose deletion hold the sweep sets:
   *
   * - an account with a billable subscription of the product and no hold in `deletion_holds`:
   *   the subscription began before the app placed holds, or the last placement failed,
   * - an account without a billable subscription of the product, with usage events that the usage
   *   push recorded, sent or held after `since`. No webhook comes for these events: a new one can
   *   need a hold, and a held one no longer counts as unbilled, so it can remove the hold.
   */
  readonly holdCandidates: (
    productId: string,
    since: Date
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
  /** The subscription of the product that changed last, for the plan page. */
  readonly latest: (
    accountId: string,
    productId: string
  ) => Effect.Effect<Subscription | undefined, SubscriptionsError>
  /** Records that the app placed a deletion hold on the account in the registry, with this reason. */
  readonly recordHold: (accountId: string, reason: string) => Effect.Effect<void, SubscriptionsError>
  /** Records that the app removed its deletion hold from the account. */
  readonly forgetHold: (accountId: string) => Effect.Effect<void, SubscriptionsError>
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
 * the usage events of the usage push, the deleted accounts that the cleanup has finished, and the
 * deletion holds that the app placed in the registry.
 * The web app, the usage push and the cleanup each create them, because each reads tables that
 * another one writes and can run first.
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
  CREATE INDEX IF NOT EXISTS polar_orders_pending ON polar_orders (account_id, created_at) WHERE status = 'pending';
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
  )`

/**
 * The usage events of account $1 that no order bills yet: not held, and sent (or recorded, when
 * not sent yet) after the creation of the newest order of the account.
 */
const unbilledUsageEvents = `
  usage_events e
  WHERE e.account_id = $1 AND e.held_at IS NULL
    AND coalesce(e.sent_at, e.created_at) > coalesce(
          (SELECT max(o.created_at) FROM polar_orders o WHERE o.account_id = $1), '-infinity'::timestamptz)`

interface SubscriptionRow {
  readonly subscription_id: string
  readonly account_id: string
  readonly product_id: string
  readonly status: string
  readonly current_period_end: Date | null
  readonly cancel_at_period_end: boolean
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
  modifiedAt: row.modified_at,
  meteredPriceIds: row.metered_price_ids,
  pastDueAt: row.past_due_at ?? undefined
})

const columns = `subscription_id, account_id::text AS account_id, product_id, status, current_period_end,
                cancel_at_period_end, modified_at, metered_price_ids, past_due_at`

const failure = (error: unknown) =>
  new SubscriptionsError({ reason: error instanceof Error ? error.message : String(error) })

/**
 * Creates the tables under the lock of the migrations, so that replicas that start together do
 * not race, and returns the queries.
 */
const make: Effect.Effect<SubscriptionsShape, SubscriptionsError, Database> =
  Effect.gen(function* () {
    const { pool, config } = yield* Database
    yield* Effect.tryPromise({
      try: () => withAdvisoryLock(config, "golem-cloud-billing/migrations", () => pool.query(tables)),
      catch: failure
    })

    const query = <R extends object>(text: string, values: ReadonlyArray<unknown> = []) =>
      Effect.tryPromise({ try: () => pool.query<R & pg.QueryResultRow>(text, [...values]), catch: failure })

    const record = (subscription: Subscription) =>
      query(
        `INSERT INTO polar_subscriptions
           (subscription_id, account_id, product_id, status, current_period_end, cancel_at_period_end, modified_at,
            metered_price_ids, past_due_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (subscription_id) DO UPDATE SET
           account_id = EXCLUDED.account_id,
           product_id = EXCLUDED.product_id,
           status = EXCLUDED.status,
           current_period_end = EXCLUDED.current_period_end,
           cancel_at_period_end = EXCLUDED.cancel_at_period_end,
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
    const pendingOrders = (accountId: string, productId: string | undefined) =>
      query<{ order_id: string; subscription_id: string; created_at: Date; metered_amount: string }>(
        `SELECT o.order_id, o.subscription_id, o.created_at,
                coalesce((SELECT sum(i.amount)
                            FROM jsonb_to_recordset(o.items) AS i("productPriceId" text, amount bigint)
                           WHERE i."productPriceId" = ANY(s.metered_price_ids)), 0)::text AS metered_amount
           FROM polar_orders o
           JOIN polar_subscriptions s ON s.subscription_id = o.subscription_id
          WHERE o.account_id = $1 AND ($2::text IS NULL OR s.product_id = $2) AND o.status = 'pending'`,
        [accountId, productId ?? null]
      ).pipe(
        Effect.map((result) =>
          result.rows.map((row) => ({
            orderId: row.order_id,
            subscriptionId: row.subscription_id,
            createdAt: row.created_at,
            meteredAmount: Number(row.metered_amount)
          }))
        )
      )

    const unbilledUsage = (accountId: string) =>
      query<{ unbilled: boolean }>(`SELECT EXISTS (SELECT 1 FROM ${unbilledUsageEvents}) AS unbilled`, [
        accountId
      ]).pipe(Effect.map((result) => result.rows[0]?.unbilled === true))

    const holdCandidates = (productId: string, since: Date) =>
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
               WHERE s.account_id = e.account_id AND s.product_id = $1 AND s.status = ANY($2))`,
        [productId, billableStatuses, since]
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

    const latest = (accountId: string, productId: string) =>
      query<SubscriptionRow>(
        `SELECT ${columns}
           FROM polar_subscriptions
          WHERE account_id = $1 AND product_id = $2
          ORDER BY modified_at DESC
          LIMIT 1`,
        [accountId, productId]
      ).pipe(Effect.map((result) => (result.rows[0] === undefined ? undefined : fromRow(result.rows[0]))))

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
      pendingOrders,
      unbilledUsage,
      holdCandidates,
      recordHold,
      forgetHold,
      ending,
      awaitingPayment,
      latest,
      isDelivered,
      markDelivered,
      withAccountLock
    }
  })

/** The subscriptions of the web app. It creates the tables after Auth has created the schema of the app. */
export const layer: Layer.Layer<Subscriptions, SubscriptionsError, Auth | Database> = Layer.effect(
  Subscriptions,
  Effect.gen(function* () {
    yield* Auth
    return yield* make
  })
)

/** The subscriptions of a command that runs without the web app, in the schema that an operator created. */
export const commandLayer: Layer.Layer<Subscriptions, SubscriptionsError, Database> = Layer.effect(
  Subscriptions,
  make
)

