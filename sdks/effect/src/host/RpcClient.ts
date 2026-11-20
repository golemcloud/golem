/** Thin injectable façade over the schema-native agent RPC host. */
import { Context, Effect, Layer } from "effect"
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
  /** Absent when the host rejected the call; `get` then fails with the rejection error. */
  readonly metadata: AgentHost.InvocationMetadata | undefined
  readonly get: () => Promise<CoreTypes.SchemaValueTree | undefined>
  readonly cancel: () => void
  readonly drop: () => void
}

export interface RpcCancellationToken {
  readonly cancel: () => void
  readonly drop: () => void
}

export interface RpcConnection {
  /** Mirrors `wasm-rpc.agent-id`. On a client of `upsert` it creates the agent when it is missing. */
  readonly agentId: () => Promise<CoreTypes.AgentId>
  readonly invokeAndAwait: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
    scopeCard?: CoreTypes.PermissionCard,
  ) => AgentHost.InvocationResultWithMetadata
  readonly invoke: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => AgentHost.InvocationMetadata
  readonly asyncInvokeAndAwait: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
    scopeCard?: CoreTypes.PermissionCard,
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

/** A connection to the agent that `create` made, with the ID that Golem issued for it. */
export interface RpcCreated {
  readonly connection: RpcConnection
  readonly agentId: CoreTypes.AgentId
}

export interface RpcClientShape {
  readonly bindSelf: () => Effect.Effect<readonly [CoreTypes.AgentId, RpcConnection], RpcHostError>
  /** Mirrors `wasm-rpc.upsert`. */
  readonly upsert: (
    agentTypeName: string,
    constructorValue: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<RpcConnection, RpcHostError>
  /** Mirrors `wasm-rpc.get-by-id`. */
  readonly getById: (
    agentTypeName: string,
    agentId: CoreTypes.AgentId,
  ) => Effect.Effect<RpcConnection, RpcHostError>
  /** Mirrors `wasm-rpc.create`. */
  readonly create: (
    agentTypeName: string,
    constructorValue: CoreTypes.SchemaValueTree,
    options: AgentHost.CreateOptions,
  ) => Effect.Effect<RpcCreated, RpcHostError>
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
  agentId: () => rpc.agentId(),
  invokeAndAwait: (methodName, input, scopeCard) =>
    rpc.invokeAndAwait(methodName, input, scopeCard),
  invoke: (methodName, input) => rpc.invoke(methodName, input, undefined),
  asyncInvokeAndAwait: (methodName, input, scopeCard) => {
    const invocation = rpc.asyncInvokeAndAwait(methodName, input, scopeCard)
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
    bindSelf: () =>
      Effect.try({
        try: () => {
          const [id, rpc] = WasmRpc.bindSelf()
          return [id, wrapWasmRpc(rpc)] as const
        },
        catch: (cause) => new RpcHostError(cause, "WasmRpc.bindSelf"),
      }),
    upsert: (agentTypeName, constructorValue) =>
      Effect.try({
        try: () => wrapWasmRpc(WasmRpc.upsert(agentTypeName, constructorValue)),
        catch: (cause) => new RpcHostError(cause, "WasmRpc.upsert"),
      }),
    getById: (agentTypeName, agentId) =>
      Effect.try({
        try: () => wrapWasmRpc(WasmRpc.getById(agentTypeName, agentId)),
        catch: (cause) => new RpcHostError(cause, "WasmRpc.getById"),
      }),
    create: (agentTypeName, constructorValue, options) =>
      Effect.tryPromise({
        try: async () => {
          const created = await WasmRpc.create(agentTypeName, constructorValue, options)
          return { connection: wrapWasmRpc(created.client), agentId: created.agentId }
        },
        catch: (cause) => new RpcHostError(cause, "WasmRpc.create"),
      }),
  }),
)
