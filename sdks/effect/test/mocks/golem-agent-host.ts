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

export const createWebhook = (): string => {
  throw new Error("createWebhook not mocked")
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
 */
export type RpcResponder = (
  call: Pick<RecordedRpcCall, "agentTypeName" | "methodName" | "input">,
) => { tag: "ok"; val: any } | { tag: "err"; val: any } | { tag: "throw"; error: any }

const recordedCalls: Array<RecordedRpcCall> = []
let responder: RpcResponder = () => ({
  tag: "throw",
  error: { tag: "remote-internal-error", val: "no responder configured" },
})
let constructorThrow: ((agentTypeName: string) => unknown | undefined) | null = null

export const __resetRpc = (): void => {
  recordedCalls.length = 0
  responder = () => ({
    tag: "throw",
    error: { tag: "remote-internal-error", val: "no responder configured" },
  })
  constructorThrow = null
  cancellationsObserved.length = 0
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

export class FutureInvokeResult {
  private resolved = false
  private result: { tag: "ok"; val: any } | { tag: "err"; val: any } | undefined
  private throwError: unknown = null
  private cancelled = false

  constructor(
    private readonly methodName: string,
    private readonly produce: () =>
      | { tag: "ok"; val: any }
      | { tag: "err"; val: any }
      | { tag: "throw"; error: any },
  ) {}

  subscribe(): MockPollable {
    return new MockPollable(() => this.ensureResolved())
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
  }

  private ensureResolved(): void {
    if (this.resolved || this.cancelled) return
    const out = this.produce()
    this.resolved = true
    if (out.tag === "throw") this.throwError = out.error
    else this.result = out
  }
}

class MockPollable {
  constructor(private readonly resolve: () => void) {}
  ready(): boolean {
    return true
  }
  block(): void {
    this.resolve()
  }
  promise(): Promise<void> {
    return Promise.resolve().then(() => this.resolve())
  }
  abortablePromise(_signal: AbortSignal): Promise<void> {
    return this.promise()
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
    // fire-and-forget: `err` and `ok` results are dropped
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
}
