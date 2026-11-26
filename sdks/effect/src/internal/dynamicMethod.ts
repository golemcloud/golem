import { Effect } from "effect"
import type { DynamicMethod } from "../DynamicClient.js"
import type { RpcConnection } from "../host/RpcClient.js"
import { awaitInvocation, scheduleInvocation, triggerInvocation } from "./rpc.js"

export const dynamicMethod = (rpc: RpcConnection, name: string): DynamicMethod => ({
  name,
  invoke: (input, options) =>
    awaitInvocation(rpc, name, Effect.succeed(input), options?.scopeCard, false).pipe(
      Effect.map(({ metadata, result }) => ({ metadata, value: result })),
    ),
  trigger: (input) => triggerInvocation(rpc, name, input, Effect.succeed),
  schedule: (at, input) => scheduleInvocation(rpc, at, name, input, Effect.succeed),
})
