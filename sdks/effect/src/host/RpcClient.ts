/** Thin injectable façade over the schema-native agent RPC host. */
import { Context, Effect, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import { WasmRpc } from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"

export class RpcHostError {
  readonly _tag = "RpcHostError"
  constructor(
    readonly cause: unknown,
    readonly operation: string,
  ) {}
}

export interface RpcInvocationHandle {
  readonly metadata: AgentHost.InvocationMetadata
  readonly get: () => Promise<CoreTypes.SchemaValueTree | undefined>
  readonly cancel: () => void
  readonly drop: () => void
}

export interface RpcCancellationToken {
  readonly cancel: () => void
  readonly drop: () => void
}

export interface RpcConnection {
  readonly invokeAndAwait: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => AgentHost.InvocationResultWithMetadata
  readonly invoke: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => AgentHost.InvocationMetadata
  readonly asyncInvokeAndAwait: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => RpcInvocationHandle
  readonly scheduleInvocation: (
    scheduledAt: AgentHost.Datetime,
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => AgentHost.ScheduledInvocationReceipt
  readonly scheduleCancelableInvocation: (
    scheduledAt: AgentHost.Datetime,
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => { readonly metadata: AgentHost.InvocationMetadata; readonly token: RpcCancellationToken }
  readonly drop: () => void
}

export interface RpcClientShape {
  readonly connect: (
    agentTypeName: string,
    constructorValue: CoreTypes.SchemaValueTree,
    phantomId: CoreTypes.Uuid | undefined,
    agentConfig: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) => Effect.Effect<RpcConnection, RpcHostError>
}

export class RpcClient extends Context.Service<RpcClient, RpcClientShape>()(
  "effect-golem/host/Rpc",
) {}

const dispose = (resource: unknown): void => {
  try {
    ;(resource as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.()
  } catch {
    // Resource release is best-effort after the operation has completed.
  }
}

const wrapWasmRpc = (rpc: WasmRpc): RpcConnection => ({
  invokeAndAwait: (methodName, input) => rpc.invokeAndAwait(methodName, input, undefined),
  invoke: (methodName, input) => rpc.invoke(methodName, input, undefined),
  asyncInvokeAndAwait: (methodName, input) => {
    const invocation = rpc.asyncInvokeAndAwait(methodName, input, undefined)
    return {
      metadata: invocation.metadata,
      get: () => invocation.future.get(),
      cancel: () => invocation.future.cancel(),
      drop: () => dispose(invocation.future),
    }
  },
  scheduleInvocation: (scheduledAt, methodName, input) =>
    rpc.scheduleInvocation(scheduledAt, methodName, input, undefined),
  scheduleCancelableInvocation: (scheduledAt, methodName, input) => {
    const receipt = rpc.scheduleCancelableInvocation(scheduledAt, methodName, input, undefined)
    return {
      metadata: receipt.metadata,
      token: {
        cancel: () => receipt.cancellationToken.cancel(),
        drop: () => dispose(receipt.cancellationToken),
      },
    }
  },
  drop: () => dispose(rpc),
})

export const RpcLive: Layer.Layer<RpcClient> = Layer.succeed(
  RpcClient,
  RpcClient.of({
    connect: (agentTypeName, constructorValue, phantomId, agentConfig) =>
      Effect.try({
        try: () =>
          wrapWasmRpc(WasmRpc.create(agentTypeName, constructorValue, phantomId, [...agentConfig])),
        catch: (cause) => new RpcHostError(cause, "WasmRpc.create"),
      }),
  }),
)
