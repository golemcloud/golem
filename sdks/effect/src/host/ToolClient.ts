import { Context, Layer } from "effect"
import * as Host from "golem:tool/host@0.1.0"

/** @since 1.6.0 @category host services */
export interface ToolClientShape {
  readonly getAllTools: typeof Host.getAllTools
  readonly getTool: typeof Host.getTool
  readonly createStdin: typeof Host.createStdin
  readonly createStdinFromStream: typeof Host.createStdinFromStream
  readonly createStdout: typeof Host.createStdout
  readonly rpc: (name: string) => Host.ToolRpc
}

/** Injectable transport for the ambient Golem tool host. @since 1.6.0 @category host services */
export class ToolClient extends Context.Service<ToolClient, ToolClientShape>()(
  "effect-golem/host/ToolClient",
) {}

/** Live tool-host layer. @since 1.6.0 @category layers */
export const ToolClientLive: Layer.Layer<ToolClient> = Layer.succeed(
  ToolClient,
  ToolClient.of({
    getAllTools: Host.getAllTools,
    getTool: Host.getTool,
    createStdin: Host.createStdin,
    createStdinFromStream: Host.createStdinFromStream,
    createStdout: Host.createStdout,
    rpc: (name) => new Host.ToolRpc(name),
  }),
)
