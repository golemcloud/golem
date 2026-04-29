/**
 * Host service for the `getConfigValue` subset of
 * `golem:agent/host@1.5.0`. Wraps the synchronous host call into an
 * Effect-typed surface so SDK code can read agent config via DI rather
 * than reaching directly into the imported namespace.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as AgentHost from "golem:agent/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"

type WitType = CoreTypes.WitType
type WitValue = CoreTypes.WitValue

export interface ConfigClientShape {
  /**
   * Mirrors `golem:agent/host.getConfigValue`. Errors thrown by the
   * host become Effect defects (caught at the call site by
   * `compileConfig` and turned into a `ConfigError({ _tag: "HostTrap" })`).
   */
  readonly getConfigValue: (key: ReadonlyArray<string>, expectedType: WitType) => WitValue
}

export class ConfigClient extends Context.Service<ConfigClient, ConfigClientShape>()(
  "effect-golem/host/Config",
) {}

export const ConfigLive: Layer.Layer<ConfigClient> = Layer.succeed(
  ConfigClient,
  ConfigClient.of({
    getConfigValue: (key, expectedType) => AgentHost.getConfigValue([...key], expectedType),
  }),
)
