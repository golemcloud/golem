import { Effect } from "effect"
import type { DynamicMethod } from "../DynamicClient.js"
import type { RpcConnection } from "../host/RpcClient.js"
import {
  awaitInvocation,
  invocationMetadataFromHost,
  scheduleCancelableInvocation,
  wrapHostThrow,
} from "./rpc.js"

export const dynamicMethod = (rpc: RpcConnection, name: string): DynamicMethod => ({
  name,
  invoke: (input, options) =>
    awaitInvocation(rpc, name, Effect.succeed(input), options?.scopeCard, false).pipe(
      Effect.map(({ metadata, result }) => ({ metadata, value: result })),
    ),
  trigger: (input) =>
    Effect.try({
      try: () => invocationMetadataFromHost(rpc.invoke(name, input)),
      catch: wrapHostThrow,
    }),
  schedule: (at, input) => scheduleCancelableInvocation(rpc, at, name, input),
})
