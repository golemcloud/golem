import { createServer } from "node:http"
import { NodeHttpServer } from "@effect/platform-node"
import { Effect, Layer } from "effect"
import { FetchHttpClient, HttpRouter } from "effect/http"
import * as Auth from "./Auth.ts"
import * as BillingConfig from "./BillingConfig.ts"
import * as Database from "./Database.ts"
import * as Http from "./Http.ts"
import * as Registry from "./Registry.ts"

/** The services of the billing app, without the configuration and the HTTP client. */
export const services = Auth.layer.pipe(
  Layer.provideMerge(Registry.layer),
  Layer.provideMerge(Database.layer)
)

const httpServer = Layer.unwrap(
  Effect.gen(function* () {
    const { httpPort } = yield* BillingConfig.BillingConfig
    return NodeHttpServer.layer(createServer, { port: httpPort })
  })
)

export const layer = HttpRouter.serve(Http.routes).pipe(
  Layer.provide(httpServer),
  Layer.provide(services),
  Layer.provide(FetchHttpClient.layer),
  Layer.provide(BillingConfig.layer)
)
