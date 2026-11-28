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
  readonly kind: "invokeAndAwait" | "trigger" | "schedule"
  readonly methodName: string
  readonly input: any
  readonly scheduledTime?: any
}

/**
 * A response handler decides what each call returns. Tests register one with
 * `__setRpcResponder`. For `trigger` and `schedule`, the response is the result that the
 * returned invocation handle awaits.
 */
export type RpcResponse =
  | { tag: "ok"; val: any }
  | { tag: "err"; val: any }
  | { tag: "throw"; error: any }

export type RpcResponder = (
  call: Pick<RecordedRpcCall, "agentTypeName" | "methodName" | "input">,
) => RpcResponse

const recordedCalls: Array<RecordedRpcCall> = []
const recordedOpens: Array<RecordedRpcOpen> = []
let responder: RpcResponder = () => ({
  tag: "throw",
  error: { tag: "remote-internal-error", val: "no responder configured" },
})
let constructorThrow: ((agentTypeName: string) => unknown | undefined) | null = null
let nextAgentId: any = { uuid: { highBits: 0x1n, lowBits: 0x8000000000000001n } }

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

const settle = (out: RpcResponse): any => {
  if (out.tag === "throw") throw out.error
  if (out.tag === "err") throw out.val
  return out.val
}

const invocationIdOf = (serial: number) => ({
  owner: { tag: "agent", val: nextAgentId },
  id: { highBits: 0n, lowBits: BigInt(serial) },
})

/** A handle of an accepted invocation whose await settles with `response`. */
export class Invocation {
  constructor(
    private readonly invocationId: any,
    private readonly response: RpcResponse = { tag: "ok", val: undefined },
  ) {}

  static get(id: any): Invocation {
    return new Invocation(id)
  }

  id(): any {
    return this.invocationId
  }

  async status(): Promise<string> {
    return "pending"
  }

  async await_(): Promise<any> {
    return settle(this.response)
  }

  async cancel(): Promise<boolean> {
    return true
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

  async invokeAndAwait(methodName: string, input: any): Promise<any> {
    const call = this.record("invokeAndAwait", methodName, input)
    const result = settle(responder({ agentTypeName: call.agentTypeName, methodName, input }))
    return {
      metadata: { agentId: nextAgentId, invocationId: invocationIdOf(recordedCalls.length) },
      result,
    }
  }

  async trigger(methodName: string, input: any): Promise<Invocation> {
    const call = this.record("trigger", methodName, input)
    const response = responder({ agentTypeName: call.agentTypeName, methodName, input })
    return new Invocation(invocationIdOf(recordedCalls.length), response)
  }

  async schedule(scheduledTime: any, methodName: string, input: any): Promise<Invocation> {
    const call = this.record("schedule", methodName, input, scheduledTime)
    const response = responder({ agentTypeName: call.agentTypeName, methodName, input })
    return new Invocation(invocationIdOf(recordedCalls.length), response)
  }
}

export const __reset = (): void => {
  registered = []
  __resetRpc()
  __resetCreateWebhookImpl()
  __resetGetConfigValueImpl()
}
