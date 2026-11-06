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
      owing: summary.owing,
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
