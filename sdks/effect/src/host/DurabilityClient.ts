/**
 * Host service for `golem:durability/durability@1.5.0`. Wraps the
 * synchronous host calls (`observe-function-call`,
 * `begin-durable-function`, `end-durable-function`,
 * `current-durable-execution-state`,
 * `persist-durable-function-invocation`,
 * `read-persisted-durable-function-invocation`) into an Effect-typed
 * surface so SDK code can reach them via DI rather than importing the
 * WIT specifier directly.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import type * as CoreTypes from "golem:core/types@1.5.0"
import * as DurabilityHost from "golem:durability/durability@1.5.0"

export interface DurabilityClientShape {
  /** Mirrors `golem:durability/durability.observe-function-call`. */
  readonly observeFunctionCall: (iface: string, function_: string) => void
  /** Mirrors `golem:durability/durability.begin-durable-function`. */
  readonly beginDurableFunction: (
    functionType: DurabilityHost.DurableFunctionType,
  ) => DurabilityHost.OplogIndex
  /** Mirrors `golem:durability/durability.end-durable-function`. */
  readonly endDurableFunction: (
    functionType: DurabilityHost.DurableFunctionType,
    beginIndex: DurabilityHost.OplogIndex,
    forcedCommit: boolean,
  ) => void
  /** Mirrors `golem:durability/durability.current-durable-execution-state`. */
  readonly currentDurableExecutionState: () => DurabilityHost.DurableExecutionState
  /** Mirrors `golem:durability/durability.persist-durable-function-invocation`. */
  readonly persistDurableFunctionInvocation: (
    functionName: string,
    request: CoreTypes.ValueAndType,
    response: CoreTypes.ValueAndType,
    functionType: DurabilityHost.DurableFunctionType,
  ) => void
  /** Mirrors `golem:durability/durability.read-persisted-durable-function-invocation`. */
  readonly readPersistedDurableFunctionInvocation: () => DurabilityHost.PersistedDurableFunctionInvocation
}

export class DurabilityClient extends Context.Service<DurabilityClient, DurabilityClientShape>()(
  "effect-golem/host/Durability",
) {}

export const DurabilityLive: Layer.Layer<DurabilityClient> = Layer.succeed(
  DurabilityClient,
  DurabilityClient.of({
    observeFunctionCall: (iface, function_) => DurabilityHost.observeFunctionCall(iface, function_),
    beginDurableFunction: (functionType) => DurabilityHost.beginDurableFunction(functionType),
    endDurableFunction: (functionType, beginIndex, forcedCommit) =>
      DurabilityHost.endDurableFunction(functionType, beginIndex, forcedCommit),
    currentDurableExecutionState: () => DurabilityHost.currentDurableExecutionState(),
    persistDurableFunctionInvocation: (functionName, request, response, functionType) =>
      DurabilityHost.persistDurableFunctionInvocation(
        functionName,
        request,
        response,
        functionType,
      ),
    readPersistedDurableFunctionInvocation: () =>
      DurabilityHost.readPersistedDurableFunctionInvocation(),
  }),
)
