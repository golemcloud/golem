/** Injectable host service for `golem:durability/durability@1.6.0`. */
import { Context, Layer } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as DurabilityHost from "golem:durability/durability@1.6.0"

export interface DurabilityClientShape {
  readonly observeFunctionCall: (iface: string, function_: string) => void
  readonly beginCustomDurableInvocationAsync: (
    functionName: string,
    request: CoreTypes.TypedSchemaValue,
    functionType: DurabilityHost.DurableFunctionType,
  ) => Promise<DurabilityHost.CustomDurableInvocation>
  /** Borrows the invocation; the caller still drops it afterwards. */
  readonly finishAsync: (
    invocation: DurabilityHost.LiveCustomDurableInvocation,
    response: CoreTypes.TypedSchemaValue,
    forcedCommit: boolean,
  ) => Promise<void>
  readonly drop: (invocation: DurabilityHost.LiveCustomDurableInvocation) => void
}

export class DurabilityClient extends Context.Service<DurabilityClient, DurabilityClientShape>()(
  "effect-golem/host/Durability",
) {}

export const DurabilityLive: Layer.Layer<DurabilityClient> = Layer.succeed(
  DurabilityClient,
  DurabilityClient.of({
    observeFunctionCall: (iface, function_) => DurabilityHost.observeFunctionCall(iface, function_),
    beginCustomDurableInvocationAsync: (functionName, request, functionType) =>
      DurabilityHost.beginCustomDurableInvocationAsync(functionName, request, functionType),
    finishAsync: (invocation, response, forcedCommit) =>
      DurabilityHost.LiveCustomDurableInvocation.finishAsync(invocation, response, forcedCommit),
    drop: (invocation) =>
      (invocation as unknown as { [Symbol.dispose]: () => void })[Symbol.dispose](),
  }),
)
