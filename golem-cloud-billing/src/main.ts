import { NodeRuntime } from "@effect/platform-node"
import { Layer, Logger } from "effect"
import * as App from "./App.ts"

App.layer.pipe(
  Layer.provide(Logger.layer([Logger.consoleJson])),
  Layer.launch,
  NodeRuntime.runMain
)
