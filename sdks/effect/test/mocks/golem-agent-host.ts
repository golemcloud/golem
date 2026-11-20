/**
 * In-memory mock for the `golem:agent/host@2.0.0` host module.
 *
 * Tests can call `__setRegisteredAgentTypes` (re-exported from this module)
 * to control what `getAllAgentTypes` returns.
 */
let registered: Array<any> = []

export const getAllAgentTypes = (): Array<any> => registered

export const getAgentType = (name: string): any =>
  registered.find((r) => r.agentType.typeName === name)

export const getAgentTypeByAgentId = (_agentId: any): any => registered[0]

let lookupAgentIdImpl: (agentTypeName: string, input: any) => any = () => undefined

export const lookupAgentId = (agentTypeName: string, input: any): any =>
  lookupAgentIdImpl(agentTypeName, input)

export const __setLookupAgentIdImpl = (fn: (agentTypeName: string, input: any) => any): void => {
  lookupAgentIdImpl = fn
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

/** How a `WasmRpc` client was opened. */
export type RpcTarget =
  | { readonly kind: "upsert"; readonly constructorValue: any }
  | { readonly kind: "getById"; readonly agentId: any }
  | { readonly kind: "create"; readonly constructorValue: any; readonly options: any }

/** A recorded static `WasmRpc` call that opens a client. */
export interface RecordedRpcOpen {
  readonly agentTypeName: string
  readonly target: RpcTarget
}

/** A single recorded call against any `WasmRpc` instance. */
export interface RecordedRpcCall {
  readonly agentTypeName: string
  readonly target: RpcTarget
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
const recordedOpens: Array<RecordedRpcOpen> = []
let responder: RpcResponder = () => ({
  tag: "throw",
  error: { tag: "remote-internal-error", val: "no responder configured" },
})
let constructorThrow: ((agentTypeName: string) => unknown | undefined) | null = null
let nextAgentId: any = { uuid: { highBits: 0x1n, lowBits: 0x8000000000000001n } }

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
  recordedOpens.length = 0
  nextAgentId = { uuid: { highBits: 0x1n, lowBits: 0x8000000000000001n } }
  lookupAgentIdImpl = () => undefined
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
export const __getRecordedRpcOpens = (): ReadonlyArray<RecordedRpcOpen> => recordedOpens

/** The agent ID that `create` returns and that `agentId()` reports for clients without one. */
export const __setNextAgentId = (id: any): void => {
  nextAgentId = id
}

export const __setRpcResponder = (r: RpcResponder): void => {
  responder = r
}

/**
 * One-shot hook that makes the next static `WasmRpc` call fail. The predicate receives the
 * agent type name and returns the `rpc-error` to throw, or `undefined` to succeed.
 */
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

  async get(): Promise<any> {
    this.ensureResolved()
    await this.readyPromise
    if (this.throwError) {
      const e = this.throwError
      this.throwError = null
      throw e
    }
    if (this.result?.tag === "err") throw this.result.val
    return this.result?.val
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
  private static open(agentTypeName: string, target: RpcTarget): WasmRpc {
    recordedOpens.push({ agentTypeName, target })
    if (constructorThrow !== null) {
      const e = constructorThrow(agentTypeName)
      constructorThrow = null
      if (e !== undefined) throw e
    }
    return new WasmRpc(agentTypeName, target)
  }

  static upsert(agentTypeName: string, constructorValue: any): WasmRpc {
    return WasmRpc.open(agentTypeName, { kind: "upsert", constructorValue })
  }

  static getById(agentTypeName: string, agentId: any): WasmRpc {
    return WasmRpc.open(agentTypeName, { kind: "getById", agentId })
  }

  static async create(agentTypeName: string, constructorValue: any, options: any) {
    const client = WasmRpc.open(agentTypeName, { kind: "create", constructorValue, options })
    return { client, agentId: nextAgentId }
  }

  private constructor(
    private readonly agentTypeName: string,
    private readonly target: RpcTarget,
  ) {}

  async agentId(): Promise<any> {
    return this.target.kind === "getById" ? this.target.agentId : nextAgentId
  }

  private record(
    kind: RecordedRpcCall["kind"],
    methodName: string,
    input: any,
    scheduledTime?: any,
  ): RecordedRpcCall {
    const call: RecordedRpcCall = {
      agentTypeName: this.agentTypeName,
      target: this.target,
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

  asyncInvokeAndAwait(methodName: string, input: any) {
    const call = this.record("asyncInvokeAndAwait", methodName, input)
    return {
      metadata: {
        agentId: nextAgentId,
        invocationId: {
          owner: { tag: "agent", val: nextAgentId },
          id: { highBits: 0n, lowBits: BigInt(recordedCalls.length) },
        },
      },
      future: new FutureInvokeResult(methodName, () =>
        responder({ agentTypeName: call.agentTypeName, methodName, input }),
      ),
    }
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
