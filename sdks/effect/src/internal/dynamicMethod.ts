import { Effect } from "effect"
import type { DynamicMethod } from "../DynamicClient.js"
import type { RpcConnection } from "../host/RpcClient.js"
import { awaitInvocation, scheduleCancelableInvocation, wrapHostThrow } from "./rpc.js"

export const dynamicMethod = (rpc: RpcConnection, name: string): DynamicMethod => ({
  name,
  invoke: (input) =>
    awaitInvocation(rpc, name, input).pipe(
      Effect.map(({ metadata, result }) => ({ metadata, value: result })),
    ),
  trigger: (input) =>
    Effect.try({
      try: () => rpc.invoke(name, input),
      catch: wrapHostThrow,
    }),
  schedule: (at, input) => scheduleCancelableInvocation(rpc, at, name, input),
})
