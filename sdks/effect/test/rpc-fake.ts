import { Effect, Layer } from "effect"
import { RpcClient, type RpcClientShape } from "../src/host/RpcClient.js"

/** An `RpcClient` layer whose operations die unless the test supplies them. */
export const rpcLayer = (shape: Partial<RpcClientShape>): Layer.Layer<RpcClient> =>
  Layer.succeed(
    RpcClient,
    RpcClient.of({
      bindSelf: () => Effect.die("unexpected self binding"),
      upsert: () => Effect.die("unexpected upsert"),
      getById: () => Effect.die("unexpected getById"),
      create: () => Effect.die("unexpected create"),
      getInvocation: () => Effect.die("unexpected getInvocation"),
      ...shape,
    }),
  )
