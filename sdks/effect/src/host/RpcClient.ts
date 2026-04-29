/**
 * Host service for the RPC subset of `golem:agent/host@1.5.0`:
 * `WasmRpc`, `FutureInvokeResult`, and `CancellationToken` resources
 * plus the typed invocation primitives `invokeAndAwait` / `invoke` /
 * `asyncInvokeAndAwait` / `scheduleCancelableInvocation`.
 *
 * Per plan §10, the service abstracts ONLY the raw constructor (`new
 * WasmRpc(...)`) and the typed invocation primitives. The Effect-side
 * cancellation machinery (`Effect.acquireUseRelease` + manual
 * `fut.subscribe()` + `pollable.abortablePromise(signal)` +
 * fiber-interrupt → host `cancel()`) stays in `src/client.ts`
 * verbatim — the wrapper methods on {@link RpcConnection} are
 * deliberately synchronous and 1:1 with the underlying WIT API so the
 * existing structure can be plumbed through unchanged.
 *
 * Errors thrown synchronously by the host — including the `RpcError`
 * variant union the WIT spec mentions on `invokeAndAwait` /
 * `asyncInvokeAndAwait` — are caught at the call site in `client.ts`
 * (via `wrapHostThrow`); the service only wraps the **constructor**
 * trap (`new WasmRpc(...)` failing before any invocation can happen)
 * into a typed {@link RpcHostError}.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as AgentHost from "golem:agent/host@1.5.0"
import { WasmRpc } from "golem:agent/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"

/**
 * Typed wrapper for synchronous host-side traps that escape the raw
 * WIT call (i.e. failures of `new WasmRpc(...)` itself). Per-method
 * RPC errors continue to be modelled by the SDK's existing
 * `RemoteCallError` typed channel — see `src/client.ts`.
 */
export class RpcHostError {
  readonly _tag = "RpcHostError"
  constructor(
    readonly cause: unknown,
    readonly operation: string,
  ) {}
}

/**
 * Sync handle wrapping a `golem:agent/host.FutureInvokeResult`.
 * Mirrors the underlying WIT shape 1:1 so consumers can keep the
 * cancellation-aware `Effect.acquireUseRelease` + abortable-promise
 * machinery they already have.
 */
export interface RpcInvocationHandle {
  /** Mirrors `FutureInvokeResult.subscribe`. May throw synchronously. */
  readonly subscribe: () => AgentHost.Pollable
  /** Mirrors `FutureInvokeResult.get`. */
  readonly get: () => AgentHost.Result<CoreTypes.DataValue, AgentHost.RpcError> | undefined
  /** Mirrors `FutureInvokeResult.cancel`. Best-effort; idempotent post-completion. */
  readonly cancel: () => void
}

/**
 * Sync handle wrapping a `golem:agent/host.CancellationToken` for
 * scheduled invocations. Best-effort: a no-op if the scheduled time
 * has already passed.
 */
export interface RpcCancellationToken {
  readonly cancel: () => void
}

/**
 * Sync surface mirroring an open `WasmRpc` resource handle. Each
 * method delegates 1:1 to the underlying WIT method so the calling
 * code in `src/client.ts` (which weaves cancellation through
 * `Effect.acquireUseRelease`) does not have to change.
 */
export interface RpcConnection {
  /** Mirrors `WasmRpc.invokeAndAwait`. Synchronous; throws an `RpcError` on failure. */
  readonly invokeAndAwait: (methodName: string, input: CoreTypes.DataValue) => CoreTypes.DataValue
  /** Mirrors `WasmRpc.invoke` (fire-and-forget). Throws synchronously on host trap. */
  readonly invoke: (methodName: string, input: CoreTypes.DataValue) => void
  /** Mirrors `WasmRpc.asyncInvokeAndAwait`. Returns a future handle. */
  readonly asyncInvokeAndAwait: (
    methodName: string,
    input: CoreTypes.DataValue,
  ) => RpcInvocationHandle
  /** Mirrors `WasmRpc.scheduleCancelableInvocation`. */
  readonly scheduleCancelableInvocation: (
    scheduledAt: AgentHost.Datetime,
    methodName: string,
    input: CoreTypes.DataValue,
  ) => RpcCancellationToken
}

export interface RpcClientShape {
  /**
   * Mirrors `new WasmRpc(target, ctorInput, phantomId, agentConfig)`.
   * Synchronous host traps thrown from the constructor become a
   * typed {@link RpcHostError}; the calling code in `src/client.ts`
   * unwraps `.cause` and routes it through the existing
   * `wrapHostThrow` to preserve the public `RemoteCallError` shape.
   *
   * Returns a fresh {@link RpcConnection}. The underlying `WasmRpc`
   * resource has no `.close()` / `dropHandle()` — JS GC reclaims it
   * once the connection goes out of scope — so this is NOT a scoped
   * Effect.
   */
  readonly connect: (
    agentTypeName: string,
    constructorValue: CoreTypes.DataValue,
    phantomId: CoreTypes.Uuid | undefined,
    agentConfig: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) => Effect.Effect<RpcConnection, RpcHostError>
}

export class RpcClient extends Context.Service<RpcClient, RpcClientShape>()(
  "effect-golem/host/Rpc",
) {}

const wrapWasmRpc = (rpc: WasmRpc): RpcConnection => ({
  invokeAndAwait: (methodName, input) => rpc.invokeAndAwait(methodName, input),
  invoke: (methodName, input) => rpc.invoke(methodName, input),
  asyncInvokeAndAwait: (methodName, input) => {
    const fut = rpc.asyncInvokeAndAwait(methodName, input)
    return {
      subscribe: () => fut.subscribe(),
      get: () => fut.get(),
      cancel: () => fut.cancel(),
    }
  },
  scheduleCancelableInvocation: (scheduledAt, methodName, input) => {
    const tok = rpc.scheduleCancelableInvocation(scheduledAt, methodName, input)
    return { cancel: () => tok.cancel() }
  },
})

export const RpcLive: Layer.Layer<RpcClient> = Layer.succeed(
  RpcClient,
  RpcClient.of({
    connect: (agentTypeName, constructorValue, phantomId, agentConfig) =>
      Effect.try({
        try: () =>
          new WasmRpc(
            agentTypeName,
            constructorValue,
            phantomId,
            agentConfig as Array<AgentCommon.TypedAgentConfigValue>,
          ),
        catch: (cause): RpcHostError => new RpcHostError(cause, "WasmRpc.constructor"),
      }).pipe(Effect.map(wrapWasmRpc)),
  }),
)
