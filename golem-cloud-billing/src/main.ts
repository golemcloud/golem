import { NodeRuntime } from "@effect/platform-node"
import { Effect, Layer, Logger } from "effect"
import { FetchHttpClient } from "effect/http"
import * as App from "./App.ts"
import * as Database from "./Database.ts"
import * as Metering from "./Metering.ts"
import * as Polar from "./Polar.ts"
import * as UsagePush from "./UsagePush.ts"

const logger = Logger.layer([Logger.consoleJson])

/**
 * `push-usage` sends the billable overage to Polar once and exits, with a nonzero exit code when it
 * fails. It reads only the database and Polar settings, so its pod needs no secret of the web app.
 */
const pushUsage = Effect.gen(function* () {
  const summary = yield* UsagePush.pushUsage(new Date())
  yield* Effect.logInfo("Pushed the billable overage to Polar").pipe(
    Effect.annotateLogs({ created: summary.created, sent: summary.sent })
  )
}).pipe(
  // The failure goes to the JSON log like every other line; runMain reports it as plain text.
  Effect.tapError((error) =>
    Effect.logError("The usage push failed; the next run sends the unsent events").pipe(
      Effect.annotateLogs({ error: error.message })
    )
  ),
  Effect.provide(
    Layer.mergeAll(Metering.layer, Polar.layer).pipe(
      Layer.provideMerge(Database.layerFromEnv),
      Layer.provide(Polar.configLayer),
      Layer.provide(FetchHttpClient.layer)
    )
  ),
  Effect.provide(logger)
)

const command = process.argv[2]
if (command === undefined) {
  App.layer.pipe(Layer.provide(logger), Layer.launch, NodeRuntime.runMain)
} else if (command === "push-usage") {
  NodeRuntime.runMain(pushUsage, { disableErrorReporting: true })
} else {
  process.stderr.write(`Unknown command: ${command}. Commands: push-usage\n`)
  process.exitCode = 2
}
