/**
 * Layer-based test fake for {@link RpcClient}. Mirrors the legacy
 * `test/mocks/golem-agent-host.ts` RPC pump (recorded calls,
 * responder dispatch, future + abortable-promise plumbing,
 * cancellation log, and the one-shot `failConstructorOnce` /
 * `failSubscribeOnce` hooks) but moves all state into per-instance
 * JS closures + Effect-typed accessors so each test gets fresh state.
 *
 * Use with `Effect.provide(eff, fake.layer)` once per test (NOT via
 * `it.layer(fake.layer)`, which would share state across the entire
 * `describe` block).
 */

import { Effect, Layer } from "effect"
import type * as AgentHost from "golem:agent/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import {
  RpcClient,
  RpcHostError,
  type RpcCancellationToken,
  type RpcConnection,
  type RpcInvocationHandle,
} from "../../src/host/RpcClient.js"

/** A single recorded call against any fake `RpcConnection` instance. */
export interface RecordedRpcCall {
  readonly agentTypeName: string
  readonly constructorValue: CoreTypes.DataValue
  readonly phantomId: CoreTypes.Uuid | undefined
  readonly agentConfig: ReadonlyArray<unknown>
  readonly kind: "invokeAndAwait" | "invoke" | "asyncInvokeAndAwait" | "schedule"
  readonly methodName: string
  readonly input: CoreTypes.DataValue
  readonly scheduledTime?: AgentHost.Datetime
}

/**
 * A response handler decides what each `*invoke*` call returns. Tests
 * register one with `setResponder`.
 *
 * Returning `{ tag: "pending" }` parks the future without resolving
 * its producer; the test then drives completion explicitly via
 * `resolvePending`. This is the canonical way to drive the
 * "fiber-interrupt cancels an in-flight invoke" path without relying
 * on `setTimeout` races.
 */
export type RpcResponse =
  | { readonly tag: "ok"; readonly val: CoreTypes.DataValue }
  | { readonly tag: "err"; readonly val: AgentHost.RpcError }
  | { readonly tag: "throw"; readonly error: unknown }
  | { readonly tag: "pending" }

export type RpcResponder = (
  call: Pick<RecordedRpcCall, "agentTypeName" | "methodName" | "input">,
) => RpcResponse

/** Eager (non-pending) shape returned to a future's producer. */
export type ResolvedRpcResponse = Exclude<RpcResponse, { tag: "pending" }>

export interface RpcFake {
  readonly layer: Layer.Layer<RpcClient>
  /** Replace the responder used by every RpcConnection produced by `connect`. */
  readonly setResponder: (r: RpcResponder) => Effect.Effect<void>
  /** Snapshot the recorded RPC calls (copy). */
  readonly getRecordedCalls: Effect.Effect<ReadonlyArray<RecordedRpcCall>>
  /** Snapshot the observed cancel() calls (copy). */
  readonly getCancellations: Effect.Effect<
    ReadonlyArray<{ readonly kind: string; readonly methodName?: string }>
  >
  /** Per-method counter for how many times a future's producer ran. */
  readonly getProduceCallCount: (methodName: string) => Effect.Effect<number>
  /**
   * Resolve the oldest still-pending future for a method. Mirrors
   * what the host would do when the remote side eventually responds.
   */
  readonly resolvePending: (
    methodName: string,
    response: ResolvedRpcResponse,
  ) => Effect.Effect<void>
  /**
   * One-shot hook to make the next `WasmRpc` constructor (i.e. the
   * next `connect` call) fail synchronously. The predicate's return
   * value (when defined) is the value thrown.
   */
  readonly failConstructorOnce: (
    predicate: (agentTypeName: string) => unknown | undefined,
  ) => Effect.Effect<void>
  /**
   * One-shot hook to make the next `FutureInvokeResult.subscribe()`
   * call throw synchronously. Lets tests exercise the SDK's
   * register-function-level try/catch guard inside `asyncInvoke`.
   */
  readonly failSubscribeOnce: (
    predicate: (methodName: string) => unknown | undefined,
  ) => Effect.Effect<void>
}

/**
 * Build a fresh fake. Use one per test — the in-memory state is owned
 * by this instance.
 */
export const make: Effect.Effect<RpcFake> = Effect.sync(() => {
  const recordedCalls: Array<RecordedRpcCall> = []
  const cancellationsObserved: Array<{ kind: string; methodName?: string }> = []
  const produceCallCounts = new Map<string, number>()
  const pendingFutures: Array<{
    methodName: string
    resolve: (response: ResolvedRpcResponse) => void
  }> = []
  let responder: RpcResponder = () => ({
    tag: "throw",
    error: { tag: "remote-internal-error", val: "no responder configured" },
  })
  let constructorThrow: ((agentTypeName: string) => unknown | undefined) | null = null
  let subscribeThrow: ((methodName: string) => unknown | undefined) | null = null

  // --- internal: mock pollable + future -----------------------------------

  class FakeFuture {
    private resolved = false
    private result:
      | { tag: "ok"; val: CoreTypes.DataValue }
      | { tag: "err"; val: AgentHost.RpcError }
      | undefined
    private throwError: unknown = null
    private cancelled = false
    private readonly readyPromise: Promise<void>
    private resolveReady: () => void = () => {}

    constructor(
      readonly methodName: string,
      private readonly produce: () => RpcResponse,
    ) {
      this.readyPromise = new Promise<void>((res) => {
        this.resolveReady = res
      })
    }

    subscribe(): AgentHost.Pollable {
      if (subscribeThrow !== null) {
        const e = subscribeThrow(this.methodName)
        subscribeThrow = null
        if (e !== undefined) throw e
      }
      return new FakePollable(this) as unknown as AgentHost.Pollable
    }

    get():
      | { tag: "ok"; val: CoreTypes.DataValue }
      | { tag: "err"; val: AgentHost.RpcError }
      | undefined {
      if (!this.resolved) return undefined
      if (this.throwError) {
        const e = this.throwError
        this.throwError = null
        throw e
      }
      return this.result
    }

    cancel(): void {
      this.cancelled = true
      cancellationsObserved.push({ kind: "async", methodName: this.methodName })
      // Unblock anyone parked on `readyPromise` (e.g. an
      // `abortablePromise` that hasn't seen its signal abort yet) so
      // tests don't leak microtasks.
      this.resolveReady()
    }

    /** @internal driven by FakePollable */
    ensureResolved(): boolean {
      if (this.resolved || this.cancelled) return this.resolved
      produceCallCounts.set(this.methodName, (produceCallCounts.get(this.methodName) ?? 0) + 1)
      const out = this.produce()
      if (out.tag === "pending") {
        pendingFutures.push({
          methodName: this.methodName,
          resolve: (final) => {
            if (this.resolved || this.cancelled) return
            if (final.tag === "throw") this.throwError = final.error
            else this.result = final
            this.resolved = true
            this.resolveReady()
          },
        })
        return false
      }
      if (out.tag === "throw") this.throwError = out.error
      else this.result = out
      this.resolved = true
      this.resolveReady()
      return true
    }

    isReady(): boolean {
      return this.resolved || this.cancelled
    }

    awaitReady(): Promise<void> {
      return this.readyPromise
    }
  }

  class FakePollable {
    constructor(private readonly fut: FakeFuture) {}
    ready(): boolean {
      this.fut.ensureResolved()
      return this.fut.isReady()
    }
    block(): void {
      this.fut.ensureResolved()
    }
    promise(): Promise<void> {
      this.fut.ensureResolved()
      if (this.fut.isReady()) return Promise.resolve()
      return this.fut.awaitReady()
    }
    /**
     * Honours the AbortSignal: rejects with an `AbortError`-shaped
     * DOMException if the signal aborts before the future resolves.
     */
    abortablePromise(signal: AbortSignal): Promise<void> {
      if (signal.aborted) {
        return Promise.reject(new DOMException("aborted", "AbortError"))
      }
      this.fut.ensureResolved()
      if (this.fut.isReady()) return Promise.resolve()
      return new Promise<void>((resolve, reject) => {
        const onAbort = (): void => {
          signal.removeEventListener("abort", onAbort)
          reject(new DOMException("aborted", "AbortError"))
        }
        signal.addEventListener("abort", onAbort, { once: true })
        this.fut.awaitReady().then(() => {
          signal.removeEventListener("abort", onAbort)
          resolve()
        })
      })
    }
  }

  // --- internal: build a single connection --------------------------------

  const makeConnection = (
    agentTypeName: string,
    constructorValue: CoreTypes.DataValue,
    phantomId: CoreTypes.Uuid | undefined,
    agentConfig: ReadonlyArray<unknown>,
  ): RpcConnection => {
    const record = (
      kind: RecordedRpcCall["kind"],
      methodName: string,
      input: CoreTypes.DataValue,
      scheduledTime?: AgentHost.Datetime,
    ): RecordedRpcCall => {
      const call: RecordedRpcCall = {
        agentTypeName,
        constructorValue,
        phantomId,
        agentConfig,
        kind,
        methodName,
        input,
        scheduledTime,
      }
      recordedCalls.push(call)
      return call
    }

    const dispatch = (
      kind: RecordedRpcCall["kind"],
      methodName: string,
      input: CoreTypes.DataValue,
    ) => {
      record(kind, methodName, input)
      return responder({ agentTypeName, methodName, input })
    }

    return {
      invokeAndAwait: (methodName, input) => {
        const out = dispatch("invokeAndAwait", methodName, input)
        if (out.tag === "throw") throw out.error
        if (out.tag === "err") throw out.val
        if (out.tag === "pending") {
          throw new Error(
            `${methodName}: synchronous invokeAndAwait cannot honour a 'pending' responder; use asyncInvokeAndAwait`,
          )
        }
        return out.val
      },
      invoke: (methodName, input) => {
        const out = dispatch("invoke", methodName, input)
        if (out.tag === "throw") throw out.error
        // fire-and-forget: ok / err / pending are dropped
      },
      asyncInvokeAndAwait: (methodName, input): RpcInvocationHandle => {
        record("asyncInvokeAndAwait", methodName, input)
        const fut = new FakeFuture(methodName, () =>
          responder({ agentTypeName, methodName, input }),
        )
        return {
          subscribe: () => fut.subscribe(),
          get: () => fut.get(),
          cancel: () => fut.cancel(),
        }
      },
      scheduleCancelableInvocation: (scheduledAt, methodName, input): RpcCancellationToken => {
        record("schedule", methodName, input, scheduledAt)
        return {
          cancel: () => {
            cancellationsObserved.push({ kind: "scheduled", methodName })
          },
        }
      },
    }
  }

  // --- exposed Layer ------------------------------------------------------

  const layer = Layer.succeed(
    RpcClient,
    RpcClient.of({
      connect: (agentTypeName, constructorValue, phantomId, agentConfig) =>
        Effect.suspend(() => {
          if (constructorThrow !== null) {
            const predicate = constructorThrow
            constructorThrow = null
            const e = predicate(agentTypeName)
            if (e !== undefined) {
              return Effect.fail(new RpcHostError(e, "WasmRpc.constructor"))
            }
          }
          const conn = makeConnection(agentTypeName, constructorValue, phantomId, agentConfig)
          return Effect.succeed(conn)
        }),
    }),
  )

  return {
    layer,
    setResponder: (r) =>
      Effect.sync(() => {
        responder = r
      }),
    getRecordedCalls: Effect.sync(() => recordedCalls.slice()),
    getCancellations: Effect.sync(() => cancellationsObserved.slice()),
    getProduceCallCount: (methodName) => Effect.sync(() => produceCallCounts.get(methodName) ?? 0),
    resolvePending: (methodName, response) =>
      Effect.sync(() => {
        const idx = pendingFutures.findIndex((f) => f.methodName === methodName)
        if (idx < 0) {
          throw new Error(
            `resolvePending: no pending future for '${methodName}' (pending: ${
              pendingFutures.map((p) => p.methodName).join(", ") || "<none>"
            })`,
          )
        }
        const [entry] = pendingFutures.splice(idx, 1)
        entry!.resolve(response)
      }),
    failConstructorOnce: (predicate) =>
      Effect.sync(() => {
        constructorThrow = predicate
      }),
    failSubscribeOnce: (predicate) =>
      Effect.sync(() => {
        subscribeThrow = predicate
      }),
  }
})
