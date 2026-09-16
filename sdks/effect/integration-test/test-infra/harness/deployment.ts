/**
 * Build + deploy of the integration-test components.
 *
 * On layer acquire: runs `golem -L build && golem -L -Y deploy`. On
 * release: nothing — the deployed components remain in the local
 * server's data directory until the server itself is torn down.
 *
 * Skipping is supported via the `Deployment.layerSkip` constructor
 * (used by `--no-build` on the CLI) — useful when iterating on the
 * harness without paying the ~10s build cost on each run.
 */
import { Console, Context, Effect, Layer } from "effect"
import { GolemCli, GolemCliError } from "./golem-cli.ts"

export interface DeploymentHandle {
  readonly built: boolean
}

export class Deployment extends Context.Service<Deployment, DeploymentHandle>()("Deployment") {}

const buildAndDeploy = Effect.gen(function* () {
  const cli = yield* GolemCli
  yield* Console.log("[deploy] golem -L build ...")
  const buildRes = yield* cli.build()
  yield* Console.log(
    `[deploy] build complete (stdout last 500 chars): ${buildRes.stdout.slice(-500)}`,
  )
  yield* Console.log("[deploy] golem -L -Y deploy ...")
  const deployRes = yield* cli.deploy()
  yield* Console.log(
    `[deploy] deploy complete (stdout last 500 chars): ${deployRes.stdout.slice(-500)}`,
  )
  return Deployment.of({ built: true })
})

export const layer: Layer.Layer<Deployment, GolemCliError, GolemCli> = Layer.effect(
  Deployment,
  buildAndDeploy,
)

export const layerSkip: Layer.Layer<Deployment> = Layer.succeed(
  Deployment,
  Deployment.of({ built: false }),
)
