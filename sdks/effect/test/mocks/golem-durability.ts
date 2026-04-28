/**
 * Runtime mock for `golem:durability/durability@1.5.0`. Implements the
 * surface actually exercised by the SDK and its tests:
 *
 * - `observe-function-call` (records the (iface, fn) pair into a list)
 * - `begin-durable-function` / `end-durable-function` (track open
 *   brackets via an in-memory stack, emit a strictly-increasing index)
 * - `current-durable-execution-state` (settable `is-live` flag, reads
 *   the current persistence level via the api/host mock)
 * - `persist-durable-function-invocation` (FIFO queue of persisted
 *   entries that the SDK can replay)
 * - `read-persisted-durable-function-invocation` (consume the FIFO)
 *
 * State is in-memory and reset via `__resetAll()`. Per-feature seeders
 * are exposed so unit tests can drive deterministic behaviour.
 *
 * `LazyInitializedPollable` is exported as a stub class so the SDK
 * type-imports keep working — it is not used by the wrapper today.
 */

import * as ApiHostMock from "./golem-api-host.js"
import type { OplogIndex } from "./golem-core-types.js"

// ---------------------------------------------------------------------------
// Re-exported WIT-shape types (mirrors `golem-types/golem_durability_*.d.ts`)
// ---------------------------------------------------------------------------

export type Pollable = unknown
export type Datetime = { seconds: bigint; nanoseconds: number }
export type ValueAndType = unknown // intentionally opaque — the SDK only round-trips this
export type PersistenceLevel = ApiHostMock.PersistenceLevel
export type WrappedFunctionType =
  | { tag: "read-local" }
  | { tag: "write-local" }
  | { tag: "read-remote" }
  | { tag: "write-remote" }
  | { tag: "write-remote-batched"; val: OplogIndex | undefined }
  | { tag: "write-remote-transaction"; val: OplogIndex | undefined }
export type DurableFunctionType = WrappedFunctionType
export type OplogEntryVersion = "v1" | "v2"

export type DurableExecutionState = {
  isLive: boolean
  persistenceLevel: PersistenceLevel
}

export type PersistedDurableFunctionInvocation = {
  timestamp: Datetime
  functionName: string
  response: ValueAndType
  functionType: DurableFunctionType
  entryVersion: OplogEntryVersion
}

// ---------------------------------------------------------------------------
// observe-function-call
// ---------------------------------------------------------------------------

const _observed: Array<[string, string]> = []

export const observeFunctionCall = (iface: string, function_: string): void => {
  _observed.push([iface, function_])
}

export const __getObservedCalls = (): ReadonlyArray<readonly [string, string]> => [..._observed]

// ---------------------------------------------------------------------------
// begin / end durable function
// ---------------------------------------------------------------------------

interface BeginCall {
  functionType: DurableFunctionType
  index: OplogIndex
}
interface EndCall {
  functionType: DurableFunctionType
  beginIndex: OplogIndex
  forcedCommit: boolean
}

let _nextDurableIndex: OplogIndex = 100n
const _begins: Array<BeginCall> = []
const _ends: Array<EndCall> = []
const _open = new Set<bigint>()

export const beginDurableFunction = (functionType: DurableFunctionType): OplogIndex => {
  const index = _nextDurableIndex
  _nextDurableIndex = _nextDurableIndex + 1n
  _begins.push({ functionType, index })
  _open.add(index)
  return index
}

export const endDurableFunction = (
  functionType: DurableFunctionType,
  beginIndex: OplogIndex,
  forcedCommit: boolean,
): void => {
  _ends.push({ functionType, beginIndex, forcedCommit })
  _open.delete(beginIndex)
}

export const __getBeginCalls = (): ReadonlyArray<BeginCall> => [..._begins]
export const __getEndCalls = (): ReadonlyArray<EndCall> => [..._ends]
export const __getOpenBrackets = (): ReadonlyArray<OplogIndex> => Array.from(_open).sort()

// ---------------------------------------------------------------------------
// current-durable-execution-state
// ---------------------------------------------------------------------------

let _isLive = true

export const currentDurableExecutionState = (): DurableExecutionState => ({
  isLive: _isLive,
  // Read the level from the api/host mock so `withPersistenceLevel`
  // composition remains visible inside `wrap`.
  persistenceLevel: ApiHostMock.getOplogPersistenceLevel(),
})

export const __setIsLive = (next: boolean): void => {
  _isLive = next
}
export const __getIsLive = (): boolean => _isLive

// ---------------------------------------------------------------------------
// persist / read-persisted
// ---------------------------------------------------------------------------

interface PersistCall {
  functionName: string
  request: ValueAndType
  response: ValueAndType
  functionType: DurableFunctionType
}

const _persisted: Array<PersistCall> = []
const _replayQueue: Array<PersistedDurableFunctionInvocation> = []

export const persistDurableFunctionInvocation = (
  functionName: string,
  request: ValueAndType,
  response: ValueAndType,
  functionType: DurableFunctionType,
): void => {
  _persisted.push({ functionName, request, response, functionType })
}

export const readPersistedDurableFunctionInvocation = (): PersistedDurableFunctionInvocation => {
  const next = _replayQueue.shift()
  if (next === undefined) {
    // Real host blocks/panics; we throw a string mirroring WIT-binding error shape.
    throw `read-persisted-durable-function-invocation called with empty replay queue`
  }
  return next
}

export const __getPersistedCalls = (): ReadonlyArray<PersistCall> => [..._persisted]
export const __seedReplay = (entry: PersistedDurableFunctionInvocation): void => {
  _replayQueue.push(entry)
}
export const __getReplayQueueLength = (): number => _replayQueue.length

// ---------------------------------------------------------------------------
// LazyInitializedPollable — stub
// ---------------------------------------------------------------------------

export class LazyInitializedPollable {
  set(_pollable: Pollable): void {
    // intentionally empty
  }
  subscribe(): Pollable {
    return undefined
  }
}

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

export const __resetAll = (): void => {
  _observed.length = 0
  _begins.length = 0
  _ends.length = 0
  _open.clear()
  _nextDurableIndex = 100n
  _isLive = true
  _persisted.length = 0
  _replayQueue.length = 0
}

export const __reset = __resetAll
