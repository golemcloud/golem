import { Cause, Effect, Layer } from "effect"
import { FetchHttpClient } from "effect/http"
import * as Cleanup from "./Cleanup.ts"
import * as Database from "./Database.ts"
import * as Metering from "./Metering.ts"
import * as Polar from "./Polar.ts"
import * as RegistryAccounts from "./RegistryAccounts.ts"
import * as Subscriptions from "./Subscriptions.ts"
import * as UsagePush from "./UsagePush.ts"

/** The message of the first failure or defect of `cause`, without a stack trace. */
const messageOf = (cause: Cause.Cause<unknown>): string => {
  const error = Cause.squash(cause)
  return typeof error === "object" && error !== null && "message" in error && typeof error.message === "string"
    ? error.message
    : String(error)
}

/**
 * `push-usage` sends the billable overage to Polar once. It reads only the database and Polar
 * settings, so its pod needs no secret of the web app.
 *
 * The command logs its failure before it fails, also a failure to read the settings or to connect
 * to the database, so the log of a failed run names the cause.
 */
export const pushUsage = Effect.gen(function* () {
  const summary = yield* UsagePush.pushUsage(new Date())
  yield* Effect.logInfo("Pushed the billable overage to Polar").pipe(
    Effect.annotateLogs({ created: summary.created, sent: summary.sent, held: summary.held })
  )
}).pipe(
  Effect.provide(
    Layer.mergeAll(Metering.layer, Polar.layer).pipe(
      Layer.provideMerge(Database.layerFromEnv),
      Layer.provide(Polar.configLayer),
      Layer.provide(FetchHttpClient.layer)
    )
  ),
  Effect.tapCause((cause) =>
    Effect.logError("The usage push failed").pipe(Effect.annotateLogs({ error: messageOf(cause) }))
  )
)

const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * `settle-usage <account ID>` records that an administrator checked the account in Polar: Polar
 * has billed, or will never bill, the usage that the usage push recorded until now (see
 * `Subscriptions.settleUsage`). The deletion hold and the cleanup then no longer count that usage.
 * It needs only the database settings, so it runs in the pod of the web app.
 */
export const settleUsage = (accountId: string | undefined) =>
  Effect.suspend((): Effect.Effect<void, unknown> =>
    accountId !== undefined && uuidPattern.test(accountId)
      ? Effect.gen(function* () {
          const subscriptions = yield* Subscriptions.Subscriptions
          const settledThrough = yield* subscriptions.settleUsage(accountId)
          yield* Effect.logInfo("Settled the recorded usage of the account").pipe(
            Effect.annotateLogs({ golemAccountId: accountId, settledThrough: settledThrough.toISOString() })
          )
        }).pipe(Effect.provide(Subscriptions.commandLayer.pipe(Layer.provide(Database.layerFromEnv))))
      : Effect.fail(new Error(`settle-usage needs the ID of a Golem account, not ${accountId ?? "nothing"}`))
  ).pipe(
    Effect.tapCause((cause) =>
      Effect.logError("The usage settlement failed").pipe(Effect.annotateLogs({ error: messageOf(cause) }))
    )
  )

/**
 * `cleanup-deleted-accounts` deletes the Polar customer and the Better Auth user of each account
 * that the registry deleted, once (see `Cleanup.ts`). Like `push-usage`, it reads only the
 * database and Polar settings. Its Polar token needs only `customers:write`.
 */
export const cleanupDeletedAccounts = Effect.gen(function* () {
  const summary = yield* Cleanup.cleanupDeletedAccounts
  yield* Effect.logInfo("Cleaned up the deleted accounts").pipe(
    Effect.annotateLogs({
      deletedCustomers: summary.deletedCustomers,
      unsettled: summary.unsettled,
      deletedUsers: summary.deletedUsers
    })
  )
}).pipe(
  Effect.provide(
    Layer.mergeAll(RegistryAccounts.layer, Subscriptions.commandLayer, Polar.layer).pipe(
      Layer.provideMerge(Database.layerFromEnv),
      Layer.provide(Polar.configLayer),
      Layer.provide(FetchHttpClient.layer)
    )
  ),
  Effect.tapCause((cause) =>
    Effect.logError("The cleanup of deleted accounts failed").pipe(Effect.annotateLogs({ error: messageOf(cause) }))
  )
)
