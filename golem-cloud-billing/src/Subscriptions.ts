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
  /** The subscriptions of the product that the account has, in every status. */
  readonly ofAccount: (
    accountId: string,
    productId: string
  ) => Effect.Effect<ReadonlyArray<Subscription>, SubscriptionsError>
  /**
   * The accounts with a billable subscription of the product that is canceled at the period end,
   * with the period end between `from` and `to`.
   */
  readonly ending: (
    productId: string,
    from: Date,
    to: Date
  ) => Effect.Effect<ReadonlyArray<string>, SubscriptionsError>
  /** The subscription of the product that changed last, for the plan page. */
  readonly latest: (
    accountId: string,
    productId: string
  ) => Effect.Effect<Subscription | undefined, SubscriptionsError>
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
 * The tables of the subscriptions. The usage push also creates them, because it reads
 * `polar_subscriptions` and can run before the web app.
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
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  );
  CREATE INDEX IF NOT EXISTS polar_subscriptions_account ON polar_subscriptions (account_id, product_id);
  CREATE TABLE IF NOT EXISTS polar_webhook_deliveries (
    webhook_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
  )`

interface SubscriptionRow {
  readonly subscription_id: string
  readonly account_id: string
  readonly product_id: string
  readonly status: string
  readonly current_period_end: Date | null
  readonly cancel_at_period_end: boolean
  readonly modified_at: Date
}

const fromRow = (row: SubscriptionRow): Subscription => ({
  subscriptionId: row.subscription_id,
  accountId: row.account_id,
  productId: row.product_id,
  status: row.status,
  currentPeriodEnd: row.current_period_end ?? undefined,
  cancelAtPeriodEnd: row.cancel_at_period_end,
  modifiedAt: row.modified_at
})

const failure = (error: unknown) =>
  new SubscriptionsError({ reason: error instanceof Error ? error.message : String(error) })

/**
 * Creates the tables after Auth has created the schema of the app, under the lock of the
 * migrations, so that replicas that start together do not race.
 */
export const layer: Layer.Layer<Subscriptions, SubscriptionsError, Auth | Database> = Layer.effect(
  Subscriptions,
  Effect.gen(function* () {
    yield* Auth
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
           (subscription_id, account_id, product_id, status, current_period_end, cancel_at_period_end, modified_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (subscription_id) DO UPDATE SET
           account_id = EXCLUDED.account_id,
           product_id = EXCLUDED.product_id,
           status = EXCLUDED.status,
           current_period_end = EXCLUDED.current_period_end,
           cancel_at_period_end = EXCLUDED.cancel_at_period_end,
           modified_at = EXCLUDED.modified_at,
           updated_at = clock_timestamp()
         WHERE polar_subscriptions.modified_at <= EXCLUDED.modified_at`,
        [
          subscription.subscriptionId,
          subscription.accountId,
          subscription.productId,
          subscription.status,
          subscription.currentPeriodEnd ?? null,
          subscription.cancelAtPeriodEnd,
          subscription.modifiedAt
        ]
      ).pipe(Effect.asVoid)

    const ofAccount = (accountId: string, productId: string) =>
      query<SubscriptionRow>(
        `SELECT subscription_id, account_id::text AS account_id, product_id, status, current_period_end,
                cancel_at_period_end, modified_at
           FROM polar_subscriptions
          WHERE account_id = $1 AND product_id = $2`,
        [accountId, productId]
      ).pipe(Effect.map((result) => result.rows.map(fromRow)))

    const ending = (productId: string, from: Date, to: Date) =>
      query<{ account_id: string }>(
        `SELECT DISTINCT account_id::text AS account_id
           FROM polar_subscriptions
          WHERE product_id = $1 AND status = ANY($2) AND cancel_at_period_end
            AND current_period_end BETWEEN $3 AND $4`,
        [productId, billableStatuses, from, to]
      ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    const latest = (accountId: string, productId: string) =>
      query<SubscriptionRow>(
        `SELECT subscription_id, account_id::text AS account_id, product_id, status, current_period_end,
                cancel_at_period_end, modified_at
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

    return { record, ofAccount, ending, latest, isDelivered, markDelivered, withAccountLock }
  })
)

