import { Data, Effect } from "effect"
import type pg from "pg"
import { Database } from "./Database.ts"
import { Polar, type PolarError } from "./Polar.ts"
import { RegistryAccounts, type RegistryAccountsError } from "./RegistryAccounts.ts"
import { holdFor } from "./Settlement.ts"
import { Subscriptions, type SubscriptionsError } from "./Subscriptions.ts"

/**
 * The cleanup after the registry deletes an account: it deletes the Polar customer and the
 * Better Auth user of the account.
 *
 * The deletion hold of `Settlement.holdFor` lets the registry delete an account only when the
 * account has no billable subscription and owes no paid overage. The cleanup checks the same
 * reasons again, because an administrator can remove the hold. An account with a reason keeps its
 * Polar customer and its user, and each run logs the warning `deleted_account_unsettled`.
 *
 * The delete of the Polar customer cancels its subscriptions at once, with no refund, and clears
 * its external ID. With no billable subscription left, nothing ends early. The cleanup deletes the
 * user only after the Polar customer, and the identity bridge refuses the user of a deleted
 * account, so a checkout of a new account starts only after the Polar customer of the deleted
 * account is gone. The table `deleted_account_cleanups` records the delete, so a later run does
 * not call Polar again. The user goes with its sessions and GitHub accounts. The subscriptions,
 * orders and usage events of the account stay.
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
   * Deleted accounts that keep their Polar customer, because they have a billable subscription or
   * owe paid overage.
   */
  readonly unsettled: number
  /** Better Auth users that this run deleted. */
  readonly deletedUsers: number
}

const failure = (error: unknown) =>
  new CleanupError({ reason: error instanceof Error ? error.message : String(error) })

export const cleanupDeletedAccounts: Effect.Effect<
  CleanupSummary,
  CleanupError | SubscriptionsError | RegistryAccountsError,
  Database | Subscriptions | RegistryAccounts | Polar
> = Effect.gen(function* () {
  const { pool } = yield* Database
  const subscriptions = yield* Subscriptions
  const registryAccounts = yield* RegistryAccounts
  const polar = yield* Polar
  const query = <R extends pg.QueryResultRow>(text: string, values: ReadonlyArray<unknown> = []) =>
    Effect.tryPromise({ try: () => pool.query<R>(text, [...values]), catch: failure })

  // Better Auth creates `user` when the web app starts; the cleanup can run before that.
  const hasUsers =
    (yield* query<{ exists: boolean }>(`SELECT to_regclass('"user"') IS NOT NULL AS exists`)).rows[0]?.exists === true
  const candidates = yield* query<{ account_id: string }>(
    `SELECT account_id::text AS account_id FROM polar_subscriptions
     ${hasUsers ? `UNION SELECT golem_account_id::text FROM "user" WHERE golem_account_id IS NOT NULL` : ""}`
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

  let deletedCustomers = 0
  let unsettled = 0
  let failed = 0
  for (const accountId of deleted.filter((accountId) => !finished.has(accountId))) {
    const reasons = holdFor(
      yield* subscriptions.ofAccount(accountId, undefined),
      yield* subscriptions.pendingOrders(accountId, undefined),
      yield* subscriptions.unbilledUsage(accountId)
    )
    if (reasons.length > 0) {
      unsettled++
      yield* Effect.logWarning("A deleted account is not settled; its Polar customer stays").pipe(
        Effect.annotateLogs({ event: "deleted_account_unsettled", golemAccountId: accountId, reasons: reasons.join(",") })
      )
      continue
    }
    const deletedCustomer = yield* polar.deleteCustomer(accountId).pipe(
      Effect.tap((result) =>
        Effect.logInfo("Deleted the Polar customer of a deleted account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, result })
        )
      ),
      Effect.andThen(
        query("INSERT INTO deleted_account_cleanups (account_id) VALUES ($1) ON CONFLICT (account_id) DO NOTHING", [
          accountId
        ])
      ),
      Effect.as(true),
      Effect.catch((error: PolarError | CleanupError) =>
        Effect.logError("Cannot delete the Polar customer of a deleted account").pipe(
          Effect.annotateLogs({ golemAccountId: accountId, error: error.message }),
          Effect.as(false)
        )
      )
    )
    if (deletedCustomer) {
      finished.add(accountId)
      deletedCustomers++
    } else {
      failed++
    }
  }

  // Sessions and GitHub accounts reference the user with ON DELETE CASCADE.
  const deletedUsers = hasUsers
    ? (yield* query(`DELETE FROM "user" WHERE golem_account_id = ANY($1::uuid[])`, [[...finished]])).rowCount ?? 0
    : 0
  if (failed > 0) {
    return yield* new CleanupError({ reason: `${failed} Polar customers of deleted accounts are not deleted` })
  }
  return { deletedCustomers, unsettled, deletedUsers }
})
