/** Injectable host service for `golem:durability/durability@1.6.0`. */
import { Context, Layer } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as DurabilityHost from "golem:durability/durability@1.6.0"

export interface DurabilityClientShape {
  readonly observeFunctionCall: (iface: string, function_: string) => void
  readonly beginCustomDurableInvocation: (
    functionName: string,
    request: CoreTypes.TypedSchemaValue,
    functionType: DurabilityHost.DurableFunctionType,
  ) => DurabilityHost.CustomDurableInvocation
  readonly finish: (
    invocation: DurabilityHost.LiveCustomDurableInvocation,
    response: CoreTypes.TypedSchemaValue,
    forcedCommit: boolean,
  ) => void
  readonly drop: (invocation: DurabilityHost.LiveCustomDurableInvocation) => void
}

export class DurabilityClient extends Context.Service<DurabilityClient, DurabilityClientShape>()(
  "effect-golem/host/Durability",
) {}

const finishedInvocations = new WeakSet<object>()

export const DurabilityLive: Layer.Layer<DurabilityClient> = Layer.succeed(
  DurabilityClient,
  DurabilityClient.of({
    observeFunctionCall: (iface, function_) => DurabilityHost.observeFunctionCall(iface, function_),
    beginCustomDurableInvocation: (functionName, request, functionType) =>
      DurabilityHost.beginCustomDurableInvocation(functionName, request, functionType),
    finish: (invocation, response, forcedCommit) => {
      DurabilityHost.LiveCustomDurableInvocation.finish(invocation, response, forcedCommit)
      finishedInvocations.add(invocation)
    },
    drop: (invocation) => {
      if (!finishedInvocations.delete(invocation)) {
        ;(invocation as unknown as { [Symbol.dispose]: () => void })[Symbol.dispose]()
      }
    },
  }),
)
