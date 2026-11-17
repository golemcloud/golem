import { Data, Effect } from "effect"
import type pg from "pg"
import { tableNames as authTables } from "./Auth.ts"
import { Database, requireTables, type SchemaError } from "./Database.ts"
import { Polar, type PolarError } from "./Polar.ts"
import { RegistryAccounts, type RegistryAccountsError } from "./RegistryAccounts.ts"
import { holdFor } from "./Settlement.ts"
import { billableStatuses, Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

/**
 * The cleanup after the registry deletes an account: it deletes the Polar customer and the
 * Better Auth user of the account.
 *
 * The deletion hold of `Settlement.holdFor` lets the registry delete an account only when the
 * account has no billable subscription, owes no paid overage and has no unfinished checkout. The
 * cleanup checks the same reasons again, because an administrator can remove the hold:
 *
 * - A billable subscription that is not canceled at the period end would charge the person again
 *   and again for an account that does not exist. The cleanup deletes the Polar customer at once,
 *   and logs the error `deleted_account_subscription_canceled`: an operator refunds what the person
 *   paid for the time after the delete. Golem loses the paid overage that Polar has not billed.
 * - Any other reason keeps the Polar customer and the user, and each run logs the warning
 *   `deleted_account_unsettled`. A subscription canceled at the period end runs to its end, and an
 *   unfinished checkout waits until Polar expires it or the app records its subscription.
 *
 * The delete of the Polar customer cancels its subscriptions at once, with no refund, and clears
 * its external ID. The cleanup deletes the user only after the Polar customer, and the identity
 * bridge refuses the user of a deleted account, so a checkout of a new account starts only after
 * the Polar customer of the deleted account is gone. The table `deleted_account_cleanups` records
 * the delete, so a later run does not call Polar again, unless the app records a billable
 * subscription that is not canceled at the period end after the delete: Polar created a new
 * customer with the external ID for it. The cleanup then deletes that customer too, and logs
 * `deleted_account_subscription_canceled`. The user goes with its sessions and GitHub accounts. The
 * subscriptions, orders and usage events of the account stay.
 *
 * The cleanup reads, decides and deletes the customer of each account under the lock of the
 * account, which the subscription webhooks take too. So the app records a subscription either
 * before the cleanup reads the account, or after the time of the delete.
 */

export class CleanupError extends Data.TaggedError("CleanupError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `The cleanup of deleted accounts failed: ${this.reason}`
  }
}

export interface CleanupSummary {
  /** Deleted accounts whose Polar customer this run deleted, or found already gone. */
  readonly deletedCustomers: number
  /**
   * Deleted accounts that keep their Polar customer, because they have a billable subscription
   * that runs to the end of the period, owe paid overage, or have an unfinished checkout.
   */
  readonly unsettled: number
  /**
   * Deleted accounts whose billable subscription that is not canceled at the period end this run
   * canceled, with a delete of the Polar customer. `deletedCustomers` counts them too.
   */
  readonly canceledSubscriptions: number
  /** Better Auth users that this run deleted. */
  readonly deletedUsers: number
}

const failure = (error: unknown) =>
  new CleanupError({ reason: error instanceof Error ? error.message : String(error) })

export const cleanupDeletedAccounts: Effect.Effect<
  CleanupSummary,
  CleanupError | SubscriptionsError | RegistryAccountsError | SchemaError,
  Database | Subscriptions | RegistryAccounts | Polar
> = Effect.gen(function* () {
  yield* requireTables(authTables)
  const { pool } = yield* Database
  const subscriptions = yield* Subscriptions
  const registryAccounts = yield* RegistryAccounts
  const polar = yield* Polar
  const query = <R extends pg.QueryResultRow>(text: string, values: ReadonlyArray<unknown> = []) =>
    Effect.tryPromise({ try: () => pool.query<R>(text, [...values]), catch: failure })

  const candidates = yield* query<{ account_id: string }>(
    `SELECT account_id::text AS account_id FROM polar_subscriptions
     UNION SELECT account_id::text FROM polar_checkouts
     UNION SELECT golem_account_id::text FROM "user" WHERE golem_account_id IS NOT NULL`
  )
  const deleted = yield* registryAccounts.deletedAmong(candidates.rows.map((row) => row.account_id))
  const finished = new Set(
    (
      yield* query<{ account_id: string }>(
        "SELECT account_id::text AS account_id FROM deleted_account_cleanups WHERE account_id = ANY($1::uuid[])",
        [deleted]
      )
    ).rows.map((row) => row.account_id)
  )

  // Deletes the Polar customer of the deleted account, and records the time of the delete. Each
  // subscription in `canceling` is billable and not canceled at the period end: when Polar had the
  // customer, the delete canceled it, and the person gets a refund from an operator. `failed`:
  // Polar or the database failed, and the next run tries again.
  const deleteCustomer = (accountId: string, canceling: ReadonlyArray<string>) =>
    polar.deleteCustomer(accountId).pipe(
      Effect.tap((result) =>
        Effect.logInfo("Deleted the Polar customer of a deleted account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, result })
        )
      ),
      Effect.map((result) => (result === "deleted" && canceling.length > 0 ? "canceled" : "deleted")),
      Effect.tap((result) =>
        result !== "canceled"
          ? Effect.void
          : Effect.logError(
              "Canceled a subscription of a deleted account; refund what the person paid for the time after the delete"
            ).pipe(
              Effect.annotateLogs({
                event: "deleted_account_subscription_canceled",
                golemAccountId: accountId,
                subscriptionIds: canceling.join(",")
              })
            )
      ),
      Effect.tap(() =>
        query(
          `INSERT INTO deleted_account_cleanups (account_id) VALUES ($1)
           ON CONFLICT (account_id) DO UPDATE SET polar_customer_deleted_at = clock_timestamp()`,
          [accountId]
        )
      ),
      Effect.catch((error: PolarError | CleanupError) =>
        Effect.logError("Cannot delete the Polar customer of a deleted account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, error: error.message }),
          Effect.as("failed" as const)
        )
      )
    )

  let deletedCustomers = 0
  let unsettled = 0
  let canceledSubscriptions = 0
  let failed = 0
  const count = (result: "canceled" | "deleted" | "failed") => {
    if (result === "failed") {
      failed++
      return
    }
    deletedCustomers++
    if (result === "canceled") {
      canceledSubscriptions++
    }
  }

  // The billable subscriptions, not canceled at the period end, that the app recorded after the
  // delete of the customer of a finished account.
  const restarted = (accountIds: ReadonlyArray<string>) =>
    query<{ account_id: string; subscription_ids: Array<string> }>(
      `SELECT c.account_id::text AS account_id,
              array_agg(s.subscription_id ORDER BY s.subscription_id) AS subscription_ids
         FROM deleted_account_cleanups c
         JOIN polar_subscriptions s ON s.account_id = c.account_id
        WHERE c.account_id = ANY($1::uuid[]) AND s.status = ANY($2) AND NOT s.cancel_at_period_end
          AND s.updated_at > c.polar_customer_deleted_at
        GROUP BY c.account_id`,
      [accountIds, billableStatuses]
    )
  for (const { account_id: accountId } of (yield* restarted([...finished])).rows) {
    const result = yield* subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        const row = (yield* restarted([accountId])).rows[0]
        return row === undefined ? undefined : yield* deleteCustomer(accountId, row.subscription_ids)
      })
    )
    if (result !== undefined) {
      count(result)
    }
  }

  for (const accountId of deleted.filter((accountId) => !finished.has(accountId))) {
    const result = yield* subscriptions.withAccountLock(
      accountId,
      Effect.gen(function* () {
        const accountSubscriptions = yield* subscriptions.ofAccount(accountId, undefined)
        const reasons = holdFor(
          accountSubscriptions,
          yield* subscriptions.unpaidOrders(accountId, undefined),
          yield* subscriptions.unbilledUsage(accountId),
          yield* subscriptions.unfinishedCheckouts(accountId)
        )
        const canceling = reasons.includes("subscription")
          ? accountSubscriptions
              .filter(
                (subscription) => billableStatuses.includes(subscription.status) && !subscription.cancelAtPeriodEnd
              )
              .map((subscription) => subscription.subscriptionId)
          : []
        if (reasons.length > 0 && canceling.length === 0) {
          yield* Effect.logWarning("A deleted account is not settled; its Polar customer stays").pipe(
            Effect.annotateLogs({
              event: "deleted_account_unsettled",
              golemAccountId: accountId,
              reasons: reasons.join(",")
            })
          )
          return "unsettled" as const
        }
        return yield* deleteCustomer(accountId, canceling)
      })
    )
    if (result === "unsettled") {
      unsettled++
      continue
    }
    if (result !== "failed") {
      finished.add(accountId)
    }
    count(result)
  }

  // The sweep of the web app skips a deleted account, so its recorded hold has no use any more.
  yield* query("DELETE FROM deletion_holds WHERE account_id = ANY($1::uuid[])", [[...finished]])
  // Sessions and GitHub accounts reference the user with ON DELETE CASCADE. The cascade runs as
  // the owner of those tables, so the role of the cleanup needs no privilege on them.
  const deletedUsers =
    (yield* query(`DELETE FROM "user" WHERE golem_account_id = ANY($1::uuid[])`, [[...finished]])).rowCount ?? 0
  if (failed > 0) {
    return yield* new CleanupError({ reason: `${failed} Polar customers of deleted accounts are not deleted` })
  }
  return { deletedCustomers, unsettled, canceledSubscriptions, deletedUsers }
})
