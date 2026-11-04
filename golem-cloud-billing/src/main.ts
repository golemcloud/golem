import { NodeRuntime } from "@effect/platform-node"
import { Effect, Layer, Logger } from "effect"
import * as App from "./App.ts"
import * as Commands from "./Commands.ts"

const logger = Logger.layer([Logger.consoleJson])

const command = process.argv[2]
if (command === undefined) {
  App.layer.pipe(Layer.provide(logger), Layer.launch, NodeRuntime.runMain)
} else if (command === "push-usage") {
  // The command logs its failure as JSON; runMain would report it again as plain text.
  NodeRuntime.runMain(Commands.pushUsage.pipe(Effect.provide(logger)), { disableErrorReporting: true })
} else {
  process.stderr.write(`Unknown command: ${command}. Commands: push-usage\n`)
  process.exitCode = 2
}
