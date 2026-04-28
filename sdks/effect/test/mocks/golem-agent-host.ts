/**
 * In-memory mock for the `golem:agent/host@1.5.0` host module.
 *
 * Tests can call `__setRegisteredAgentTypes` (re-exported from this module)
 * to control what `getAllAgentTypes` returns.
 */
let registered: Array<any> = []

export const getAllAgentTypes = (): Array<any> => registered

export const getAgentType = (name: string): any =>
  registered.find((r) => r.agentType.typeName === name)

export const makeAgentId = (): string => {
  throw new Error("makeAgentId not mocked")
}

export const parseAgentId = (): never => {
  throw new Error("parseAgentId not mocked")
}

/**
 * Settable mock for the host's `create-webhook`. Tests register a
 * responder via {@link __setCreateWebhookImpl}; the default throws so
 * forgetting to set it surfaces clearly.
 */
let createWebhookImpl: (id: any) => string = () => {
  throw new Error("createWebhook not mocked")
}

export const createWebhook = (id: any): string => createWebhookImpl(id)

export const __setCreateWebhookImpl = (fn: (id: any) => string): void => {
  createWebhookImpl = fn
}

export const __resetCreateWebhookImpl = (): void => {
  createWebhookImpl = () => {
    throw new Error("createWebhook not mocked")
  }
}

/**
 * Settable mock for the host's `getConfigValue`. Tests register a
 * responder via {@link __setGetConfigValueImpl}; the default throws so
 * forgetting to set it surfaces clearly.
 */
let getConfigValueImpl: (key: Array<string>, expectedType: any) => any = () => {
  throw new Error("getConfigValue not mocked")
}

export const getConfigValue = (key: Array<string>, expectedType: any): any =>
  getConfigValueImpl(key, expectedType)

export const __setGetConfigValueImpl = (
  fn: (key: Array<string>, expectedType: any) => any,
): void => {
  getConfigValueImpl = fn
}

export const __resetGetConfigValueImpl = (): void => {
  getConfigValueImpl = () => {
    throw new Error("getConfigValue not mocked")
  }
}

export const __setRegisteredAgentTypes = (types: Array<any>): void => {
  registered = types
}

// ---------------------------------------------------------------------------
// RPC mock
// ---------------------------------------------------------------------------

/** A single recorded call against any `WasmRpc` instance. */
export interface RecordedRpcCall {
  readonly agentTypeName: string
  readonly constructorValue: any
  readonly phantomId: any
  readonly agentConfig: ReadonlyArray<any>
  readonly kind: "invokeAndAwait" | "invoke" | "asyncInvokeAndAwait" | "schedule"
  readonly methodName: string
  readonly input: any
  readonly scheduledTime?: any
}

/**
 * A response handler decides what each `*invoke*` call returns. Tests
 * register one with `__setRpcResponder`.
 *
 * Returning `{ tag: "pending" }` parks the future without resolving
 * `produce`; the test then drives completion explicitly via
 * {@link __resolveRpcPending}. This is the canonical way to drive the
 * "fiber-interrupt cancels an in-flight invoke" path without relying
 * on `setTimeout` races.
 */
export type RpcResponse =
  | { tag: "ok"; val: any }
  | { tag: "err"; val: any }
  | { tag: "throw"; error: any }
  | { tag: "pending" }

export type RpcResponder = (
  call: Pick<RecordedRpcCall, "agentTypeName" | "methodName" | "input">,
) => RpcResponse

/** Eager (non-pending) shape returned to a `FutureInvokeResult`'s `produce`. */
export type ResolvedRpcResponse = Exclude<RpcResponse, { tag: "pending" }>

const recordedCalls: Array<RecordedRpcCall> = []
let responder: RpcResponder = () => ({
  tag: "throw",
  error: { tag: "remote-internal-error", val: "no responder configured" },
})
let constructorThrow: ((agentTypeName: string) => unknown | undefined) | null = null

const pendingFutures: Array<{
  methodName: string
  resolve: (response: ResolvedRpcResponse) => void
}> = []

/**
 * Resolve the oldest still-pending `FutureInvokeResult` for a method.
 * Mirrors what the host would do when the remote side eventually
 * responds. Tests use this together with a responder that returns
 * `{ tag: "pending" }`.
 */
export const __resolveRpcPending = (methodName: string, response: ResolvedRpcResponse): void => {
  const idx = pendingFutures.findIndex((f) => f.methodName === methodName)
  if (idx < 0) {
    throw new Error(
      `__resolveRpcPending: no pending future for '${methodName}' (pending: ${pendingFutures.map((p) => p.methodName).join(", ") || "<none>"})`,
    )
  }
  const [entry] = pendingFutures.splice(idx, 1)
  entry!.resolve(response)
}

export const __getPendingFutureCount = (): number => pendingFutures.length

/**
 * One-shot hook to make the next `FutureInvokeResult.subscribe()`
 * call throw synchronously. Lets tests exercise the SDK's
 * register-function-level try/catch guard inside `asyncInvoke`
 * (around `fut.subscribe()` / `pollable.abortablePromise(...)`).
 */
let subscribeThrow: ((methodName: string) => unknown | undefined) | null = null

export const __failSubscribeOnce = (
  predicate: (methodName: string) => unknown | undefined,
): void => {
  subscribeThrow = predicate
}

export const __resetRpc = (): void => {
  recordedCalls.length = 0
  responder = () => ({
    tag: "throw",
    error: { tag: "remote-internal-error", val: "no responder configured" },
  })
  constructorThrow = null
  cancellationsObserved.length = 0
  pendingFutures.length = 0
  produceCallCounts.clear()
  subscribeThrow = null
}

export const __getRecordedRpcCalls = (): ReadonlyArray<RecordedRpcCall> => recordedCalls

export const __setRpcResponder = (r: RpcResponder): void => {
  responder = r
}

export const __failConstructorOnce = (
  predicate: (agentTypeName: string) => unknown | undefined,
): void => {
  constructorThrow = predicate
}

const cancellationsObserved: Array<{ kind: string; methodName?: string }> = []
export const __getCancellations = (): ReadonlyArray<{
  kind: string
  methodName?: string
}> => cancellationsObserved

export class CancellationToken {
  constructor(private readonly methodName: string) {}
  cancel(): void {
    cancellationsObserved.push({ kind: "scheduled", methodName: this.methodName })
  }
}

/**
 * Per-method counter for how many times a `FutureInvokeResult`'s
 * `produce` callback has been invoked. Reset together with the rest
 * of the RPC mock state. Tests use this to assert "after fiber
 * interrupt + late `__resolveRpcPending`, the producer ran exactly
 * once" — i.e. the post-interrupt resolution didn't sneak through and
 * fire a second `Effect.succeed` into the resumed effect.
 */
const produceCallCounts = new Map<string, number>()

export const __getProduceCallCount = (methodName: string): number =>
  produceCallCounts.get(methodName) ?? 0

export class FutureInvokeResult {
  private resolved = false
  private result: { tag: "ok"; val: any } | { tag: "err"; val: any } | undefined
  private throwError: unknown = null
  private cancelled = false
  private readonly readyPromise: Promise<void>
  private resolveReady: () => void = () => {}

  constructor(
    private readonly methodName: string,
    private readonly produce: () => RpcResponse,
  ) {
    this.readyPromise = new Promise<void>((res) => {
      this.resolveReady = res
    })
  }

  subscribe(): MockPollable {
    if (subscribeThrow !== null) {
      const e = subscribeThrow(this.methodName)
      subscribeThrow = null
      if (e !== undefined) throw e
    }
    return new MockPollable(this)
  }

  get(): { tag: "ok"; val: any } | { tag: "err"; val: any } | undefined {
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
    // tests don't leak microtasks. The post-cancel `get()` returns
    // undefined which the SDK reports as a `RemoteResponseError`.
    this.resolveReady()
  }

  /** @internal called by `MockPollable` to drive the producer lazily. */
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

  /** @internal */
  isReady(): boolean {
    return this.resolved || this.cancelled
  }

  /** @internal */
  awaitReady(): Promise<void> {
    return this.readyPromise
  }
}

class MockPollable {
  constructor(private readonly fut: FutureInvokeResult) {}
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
   * Mirrors the wasm-rquickjs extension that the real host exposes.
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

export class WasmRpc {
  constructor(
    private readonly agentTypeName: string,
    private readonly constructorValue: any,
    private readonly phantomId: any,
    private readonly agentConfig: ReadonlyArray<any>,
  ) {
    if (constructorThrow !== null) {
      const e = constructorThrow(agentTypeName)
      constructorThrow = null
      if (e !== undefined) throw e
    }
  }

  private record(
    kind: RecordedRpcCall["kind"],
    methodName: string,
    input: any,
    scheduledTime?: any,
  ): RecordedRpcCall {
    const call: RecordedRpcCall = {
      agentTypeName: this.agentTypeName,
      constructorValue: this.constructorValue,
      phantomId: this.phantomId,
      agentConfig: this.agentConfig,
      kind,
      methodName,
      input,
      scheduledTime,
    }
    recordedCalls.push(call)
    return call
  }

  invokeAndAwait(methodName: string, input: any): any {
    const call = this.record("invokeAndAwait", methodName, input)
    const out = responder({
      agentTypeName: call.agentTypeName,
      methodName,
      input,
    })
    if (out.tag === "throw") throw out.error
    if (out.tag === "err") throw out.val
    if (out.tag === "pending") {
      throw new Error(
        `${methodName}: synchronous invokeAndAwait cannot honour a 'pending' responder; use asyncInvokeAndAwait`,
      )
    }
    return out.val
  }

  invoke(methodName: string, input: any): void {
    const call = this.record("invoke", methodName, input)
    const out = responder({
      agentTypeName: call.agentTypeName,
      methodName,
      input,
    })
    if (out.tag === "throw") throw out.error
    // fire-and-forget: `err`, `ok`, `pending` results are dropped
  }

  asyncInvokeAndAwait(methodName: string, input: any): FutureInvokeResult {
    const call = this.record("asyncInvokeAndAwait", methodName, input)
    return new FutureInvokeResult(methodName, () =>
      responder({
        agentTypeName: call.agentTypeName,
        methodName,
        input,
      }),
    )
  }

  scheduleInvocation(scheduledTime: any, methodName: string, input: any): void {
    this.record("schedule", methodName, input, scheduledTime)
  }

  scheduleCancelableInvocation(
    scheduledTime: any,
    methodName: string,
    input: any,
  ): CancellationToken {
    this.record("schedule", methodName, input, scheduledTime)
    return new CancellationToken(methodName)
  }
}

export const __reset = (): void => {
  registered = []
  __resetRpc()
  __resetCreateWebhookImpl()
  __resetGetConfigValueImpl()
}
