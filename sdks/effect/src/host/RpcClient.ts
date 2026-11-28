/** Thin injectable façade over the schema-native agent RPC host. */
import { Context, Effect, Layer } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import { Invocation, WasmRpc } from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { withSignal } from "wasm-rquickjs:import-abort"

export class RpcHostError {
  readonly _tag = "RpcHostError"
  constructor(
    readonly cause: unknown,
    readonly operation: string,
  ) {}
}

/** The handle of an accepted invocation. Mirrors the WIT `invocation` resource. */
export interface RpcInvocation {
  readonly id: CoreTypes.InvocationId
  readonly status: () => Promise<AgentHost.InvocationStatus>
  /**
   * Mirrors `invocation.await`. Aborting `signal` drops the call; the invocation keeps running,
   * because the host treats a dropped await as a detach.
   */
  readonly await: (signal: AbortSignal) => Promise<CoreTypes.SchemaValueTree | undefined>
  readonly cancel: () => Promise<boolean>
  readonly drop: () => void
}

export interface RpcConnection {
  /** Mirrors `wasm-rpc.agent-id`. On a client of `upsert` it creates the agent when it is missing. */
  readonly agentId: () => Promise<CoreTypes.AgentId>
  /**
   * Mirrors `wasm-rpc.invoke-and-await`. Aborting `signal` after the call started drops the
   * call, which withdraws the invocation on a best-effort basis while it has not started.
   */
  readonly invokeAndAwait: (
    methodName: string,
    input: CoreTypes.SchemaValueTree,
    scopeCard: CoreTypes.PermissionCard | undefined,
    signal: AbortSignal,
  ) => Promise<AgentHost.InvocationResultWithMetadata>
  /** Mirrors `wasm-rpc.trigger`. */
  readonly trigger: (methodName: string, input: CoreTypes.SchemaValueTree) => Promise<RpcInvocation>
  /** Mirrors `wasm-rpc.schedule`. */
  readonly schedule: (
    scheduledAt: AgentHost.Datetime,
    methodName: string,
    input: CoreTypes.SchemaValueTree,
  ) => Promise<RpcInvocation>
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
  /** Mirrors `invocation.get`: the handle of the invocation with the given ID, without a remote call. */
  readonly getInvocation: (id: CoreTypes.InvocationId) => Effect.Effect<RpcInvocation>
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

const wrapInvocation = (invocation: AgentHost.Invocation): RpcInvocation => ({
  id: invocation.id(),
  status: () => invocation.status(),
  await: (signal) => withSignal(signal, () => invocation.await_()),
  cancel: () => invocation.cancel(),
  drop: () => dispose(invocation),
})

const wrapWasmRpc = (rpc: WasmRpc): RpcConnection => ({
  agentId: () => rpc.agentId(),
  invokeAndAwait: (methodName, input, scopeCard, signal) =>
    withSignal(signal, () => rpc.invokeAndAwait(methodName, input, scopeCard)),
  trigger: async (methodName, input) => wrapInvocation(await rpc.trigger(methodName, input)),
  schedule: async (scheduledAt, methodName, input) =>
    wrapInvocation(await rpc.schedule(scheduledAt, methodName, input)),
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
    getInvocation: (id) => Effect.sync(() => wrapInvocation(Invocation.get(id))),
  }),
)
