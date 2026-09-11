/**
 * Host service for `wasi:cli/environment@0.2.3`. Wraps the
 * `getEnvironment()` host call so SDK code can read the process
 * environment via Effect-typed DI rather than a direct ESM specifier
 * import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer } from "effect"
import * as WasiEnv from "wasi:cli/environment@0.2.3"

export interface EnvironmentClientShape {
  readonly getEnvironment: Effect.Effect<ReadonlyArray<readonly [string, string]>>
}

export class EnvironmentClient extends Context.Service<EnvironmentClient, EnvironmentClientShape>()(
  "effect-golem/host/Environment",
) {}

export const EnvironmentLive: Layer.Layer<EnvironmentClient> = Layer.succeed(
  EnvironmentClient,
  EnvironmentClient.of({
    getEnvironment: Effect.sync(() => WasiEnv.getEnvironment()),
  }),
)
