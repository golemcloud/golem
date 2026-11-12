import type { TypedSchemaValue } from "golem:core/types@2.0.0"

export type DurableFunctionType =
  | { tag: "read-local" }
  | { tag: "write-local" }
  | { tag: "read-remote" }
  | { tag: "write-remote" }
export type OplogEntryVersion = "v1" | "v2"
export type PersistedDurableFunctionInvocation = {
  timestamp: { seconds: bigint; nanoseconds: number }
  functionName: string
  response: TypedSchemaValue
  functionType: DurableFunctionType
  entryVersion: OplogEntryVersion
}

let live = true
let replay: PersistedDurableFunctionInvocation[] = []
let observed: Array<[string, string]> = []
let begins: Array<{
  functionName: string
  request: TypedSchemaValue
  functionType: DurableFunctionType
}> = []
let finishes: Array<{ functionName: string; response: TypedSchemaValue; forcedCommit: boolean }> =
  []
let drops = 0
let events: string[] = []

/** Appends to the ordered host event log shared with the runtime context mock. */
export const __record = (event: string): void => {
  events.push(event)
}

export class LiveCustomDurableInvocation {
  finished = false
  disposed = false
  constructor(readonly functionName: string) {}
  enter(): void {
    __record(`enter:${this.functionName}`)
  }
  static finishAsync(
    invocation: LiveCustomDurableInvocation,
    response: TypedSchemaValue,
    forcedCommit: boolean,
  ): Promise<void> {
    if (invocation.disposed) return Promise.reject(new Error("finish after drop"))
    __record(`finish:${invocation.functionName}`)
    return Promise.resolve().then(() => {
      invocation.finished = true
      finishes.push({ functionName: invocation.functionName, response, forcedCommit })
    })
  }
  [Symbol.dispose](): void {
    if (this.disposed) throw new Error("double drop")
    this.disposed = true
    __record(`drop:${this.functionName}`)
    if (!this.finished) drops++
  }
}

export const observeFunctionCall = (iface: string, function_: string): void => {
  observed.push([iface, function_])
}

export const beginCustomDurableInvocationAsync = (
  functionName: string,
  request: TypedSchemaValue,
  functionType: DurableFunctionType,
) => {
  begins.push({ functionName, request, functionType })
  __record(`begin:${functionName}`)
  if (live)
    return Promise.resolve({
      tag: "live",
      val: new LiveCustomDurableInvocation(functionName),
    } as const)
  const entry = replay.shift()
  if (entry === undefined) return Promise.reject(new Error("empty replay queue"))
  return Promise.resolve({ tag: "replayed", val: entry } as const)
}

export const __setIsLive = (value: boolean): void => {
  live = value
}
export const __seedReplay = (entry: PersistedDurableFunctionInvocation): void => {
  replay.push(entry)
}
export const __getObservedCalls = () => [...observed]
export const __getBeginCalls = () => [...begins]
export const __getFinishCalls = () => [...finishes]
/** Live invocations dropped without a completed finish. */
export const __getDrops = () => drops
export const __getEvents = () => [...events]
export const __resetAll = (): void => {
  live = true
  replay = []
  observed = []
  begins = []
  finishes = []
  drops = 0
  events = []
}
export const __reset = __resetAll
