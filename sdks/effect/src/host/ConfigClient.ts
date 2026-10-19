import { Context, Layer } from "effect"
import * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"

export interface ConfigClientShape {
  readonly getConfigValue: (
    key: ReadonlyArray<string>,
    expected: CoreTypes.SchemaGraph,
  ) => CoreTypes.SchemaValueTree
}

export class ConfigClient extends Context.Service<ConfigClient, ConfigClientShape>()(
  "effect-golem/host/Config",
) {}

export const ConfigLive: Layer.Layer<ConfigClient> = Layer.succeed(
  ConfigClient,
  ConfigClient.of({
    getConfigValue: (key, expected) => AgentHost.getConfigValue([...key], expected),
  }),
)
