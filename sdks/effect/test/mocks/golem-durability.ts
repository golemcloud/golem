import type { TypedSchemaValue } from "golem:core/types@2.0.0"
import type { OplogIndex } from "./golem-core-types.js"

export type DurableFunctionType =
  | { tag: "read-local" }
  | { tag: "write-local" }
  | { tag: "read-remote" }
  | { tag: "write-remote" }
  | { tag: "write-remote-batched"; val: OplogIndex | undefined }
  | { tag: "write-remote-transaction"; val: OplogIndex | undefined }
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
let finishes: Array<{ response: TypedSchemaValue; forcedCommit: boolean }> = []
let drops = 0

export class LiveCustomDurableInvocation {
  consumed = false
  static finish(
    invocation: LiveCustomDurableInvocation,
    response: TypedSchemaValue,
    forcedCommit: boolean,
  ): void {
    invocation.consumed = true
    finishes.push({ response, forcedCommit })
  }
  [Symbol.dispose](): void {
    if (!this.consumed) {
      this.consumed = true
      drops++
    }
  }
}

export const observeFunctionCall = (iface: string, function_: string): void => {
  observed.push([iface, function_])
}
export const beginCustomDurableInvocation = (
  functionName: string,
  request: TypedSchemaValue,
  functionType: DurableFunctionType,
) => {
  begins.push({ functionName, request, functionType })
  if (live) return { tag: "live", val: new LiveCustomDurableInvocation() } as const
  const entry = replay.shift()
  if (entry === undefined) throw new Error("empty replay queue")
  return { tag: "replayed", val: entry } as const
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
export const __getDrops = () => drops
export const __resetAll = (): void => {
  live = true
  replay = []
  observed = []
  begins = []
  finishes = []
  drops = 0
}
export const __reset = __resetAll
