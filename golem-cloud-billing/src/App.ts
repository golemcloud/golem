import { createServer } from "node:http"
import { NodeHttpServer } from "@effect/platform-node"
import { Effect, Layer } from "effect"
import { FetchHttpClient, HttpRouter } from "effect/http"
import * as Auth from "./Auth.ts"
import * as BillingConfig from "./BillingConfig.ts"
import * as Database from "./Database.ts"
import * as Http from "./Http.ts"
import * as Polar from "./Polar.ts"
import * as Registry from "./Registry.ts"
import * as Subscriptions from "./Subscriptions.ts"

/** The web app calls Polar with the token of `BillingConfig`, which can create checkouts and sessions. */
const polarConfig = Layer.effect(
  Polar.PolarConfig,
  Effect.gen(function* () {
    const { polar } = yield* BillingConfig.BillingConfig
    return { apiUrl: polar.apiUrl, accessToken: polar.accessToken }
  })
)

/** The services of the billing app, without the configuration and the HTTP client. */
export const services = Layer.mergeAll(Subscriptions.layer, Polar.layer.pipe(Layer.provide(polarConfig))).pipe(
  Layer.provideMerge(Auth.layer),
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
