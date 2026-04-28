/**
 * Runtime mock for `golem:api/host@1.5.0`. Implements the slice of the
 * host API actually exercised by the SDK and its tests:
 *
 * - idempotency-key generator (used by `src/client.ts`)
 * - persistence-level / idempotence-mode / atomic-region accessors
 *   (used by `src/durability.ts`)
 * - oplog index, oplog commit (used by `src/durability.ts` /
 *   `src/oplog.ts`)
 * - self / agent metadata + lifecycle controls (used by
 *   `src/agents.ts`)
 * - promise rendezvous (used by `src/agents.ts`)
 *
 * State is in-memory and reset via `__resetAll()` (also exported as
 * `__reset` for parity with the other mocks). Per-feature seeders are
 * exposed so unit tests can drive deterministic behaviour.
 */

import {
  type AgentId,
  type ComponentId,
  type OplogIndex,
  type Uuid,
  uuidToString,
} from "./golem-core-types.js"

// ---------------------------------------------------------------------------
// Re-exported WIT-shape types
// ---------------------------------------------------------------------------

export type ComponentRevision = bigint
export type EnvironmentId = { uuid: Uuid }
export type AgentStatus =
  | "running"
  | "idle"
  | "suspended"
  | "interrupted"
  | "retrying"
  | "failed"
  | "exited"
export type FilterComparator =
  | "equal"
  | "not-equal"
  | "greater-equal"
  | "greater"
  | "less-equal"
  | "less"
export type StringFilterComparator = "equal" | "not-equal" | "like" | "not-like" | "starts-with"
export type AgentMetadata = {
  agentId: AgentId
  args: string[]
  env: [string, string][]
  config: [string, string][]
  status: AgentStatus
  componentRevision: bigint
  retryCount: bigint
  environmentId: EnvironmentId
}
export type PersistenceLevel =
  | { tag: "persist-nothing" }
  | { tag: "persist-remote-side-effects" }
  | { tag: "smart" }
export type UpdateMode = "automatic" | "snapshot-based"
export type RevertAgentTarget =
  | { tag: "revert-to-oplog-index"; val: OplogIndex }
  | { tag: "revert-last-invocations"; val: bigint }
export type ForkDetails = { forkedPhantomId: Uuid }
export type ForkResult = { tag: "original"; val: ForkDetails } | { tag: "forked"; val: ForkDetails }
export type AgentNameFilter = { comparator: StringFilterComparator; value: string }
export type AgentStatusFilter = { comparator: FilterComparator; value: AgentStatus }
export type AgentVersionFilter = { comparator: FilterComparator; value: bigint }
export type AgentCreatedAtFilter = { comparator: FilterComparator; value: bigint }
export type AgentEnvFilter = {
  name: string
  comparator: StringFilterComparator
  value: string
}
export type AgentConfigVarsFilter = {
  name: string
  comparator: StringFilterComparator
  value: string
}
export type AgentPropertyFilter =
  | { tag: "name"; val: AgentNameFilter }
  | { tag: "status"; val: AgentStatusFilter }
  | { tag: "version"; val: AgentVersionFilter }
  | { tag: "created-at"; val: AgentCreatedAtFilter }
  | { tag: "env"; val: AgentEnvFilter }
  | { tag: "config"; val: AgentConfigVarsFilter }
export type AgentAllFilter = { filters: AgentPropertyFilter[] }
export type AgentAnyFilter = { filters: AgentAllFilter[] }
export type Snapshot = { payload: Uint8Array; mimeType: string }

// ---------------------------------------------------------------------------
// Idempotency key generator
// ---------------------------------------------------------------------------

let counter = 0n

export const __resetIdempotency = (): void => {
  counter = 0n
}

/** Returns a deterministic, unique UUID per call (counter-based). */
export const generateIdempotencyKey = (): Uuid => {
  counter += 1n
  return { highBits: 0n, lowBits: counter }
}

export const __nextIdempotencyKeyAsString = (): string => uuidToString(generateIdempotencyKey())

// ---------------------------------------------------------------------------
// Persistence level / idempotence mode
// ---------------------------------------------------------------------------

let _persistenceLevel: PersistenceLevel = { tag: "smart" }
let _idempotenceMode = true

export const getOplogPersistenceLevel = (): PersistenceLevel => _persistenceLevel
export const setOplogPersistenceLevel = (next: PersistenceLevel): void => {
  _persistenceLevel = next
}
export const getIdempotenceMode = (): boolean => _idempotenceMode
export const setIdempotenceMode = (next: boolean): void => {
  _idempotenceMode = next
}

// ---------------------------------------------------------------------------
// Oplog index / atomic regions / oplog commit
// ---------------------------------------------------------------------------

let _oplogIndex: OplogIndex = 0n
const _atomicMarks: OplogIndex[] = []
const _oplogCommits: number[] = []

export const getOplogIndex = (): OplogIndex => {
  _oplogIndex = _oplogIndex + 1n
  return _oplogIndex
}

export const setOplogIndex = (idx: OplogIndex): void => {
  _oplogIndex = idx
}

export const oplogCommit = (replicas: number): void => {
  _oplogCommits.push(replicas)
}

export const markBeginOperation = (): OplogIndex => {
  _oplogIndex = _oplogIndex + 1n
  _atomicMarks.push(_oplogIndex)
  return _oplogIndex
}

export const markEndOperation = (begin: OplogIndex): void => {
  // Idempotent: removing a mark that's already gone is a no-op.
  const idx = _atomicMarks.indexOf(begin)
  if (idx >= 0) _atomicMarks.splice(idx, 1)
}

export const __getOplogIndex = (): OplogIndex => _oplogIndex
export const __setOplogIndex = (next: OplogIndex): void => {
  _oplogIndex = next
}
export const __getAtomicMarks = (): ReadonlyArray<OplogIndex> => [..._atomicMarks]
export const __getOplogCommits = (): ReadonlyArray<number> => [..._oplogCommits]

// ---------------------------------------------------------------------------
// Self metadata + agent lifecycle
// ---------------------------------------------------------------------------

const defaultComponentId: ComponentId = { uuid: { highBits: 0n, lowBits: 1n } }
const defaultEnvironmentId: EnvironmentId = { uuid: { highBits: 0n, lowBits: 1n } }

let _selfMetadata: AgentMetadata = {
  agentId: { componentId: defaultComponentId, agentId: "Test()" },
  args: [],
  env: [],
  config: [],
  status: "running",
  componentRevision: 0n,
  retryCount: 0n,
  environmentId: defaultEnvironmentId,
}

const _otherMetadata = new Map<string, AgentMetadata>()

export const getSelfMetadata = (): AgentMetadata => _selfMetadata
export const __setSelfMetadata = (next: AgentMetadata): void => {
  _selfMetadata = next
}

const agentIdKey = (id: AgentId): string => `${uuidToString(id.componentId.uuid)}::${id.agentId}`

export const getAgentMetadata = (id: AgentId): AgentMetadata | undefined =>
  _otherMetadata.get(agentIdKey(id))
export const __setAgentMetadata = (id: AgentId, meta: AgentMetadata | undefined): void => {
  if (meta === undefined) _otherMetadata.delete(agentIdKey(id))
  else _otherMetadata.set(agentIdKey(id), meta)
}

interface UpdateAgentCall {
  agentId: AgentId
  targetRevision: ComponentRevision
  mode: UpdateMode
}
const _updateCalls: UpdateAgentCall[] = []
export const updateAgent = (
  agentId: AgentId,
  targetRevision: ComponentRevision,
  mode: UpdateMode,
): void => {
  _updateCalls.push({ agentId, targetRevision, mode })
}
export const __getUpdateCalls = (): ReadonlyArray<UpdateAgentCall> => [..._updateCalls]

interface ForkAgentCall {
  source: AgentId
  target: AgentId
  oplogIdxCutOff: OplogIndex
}
const _forkCalls: ForkAgentCall[] = []
export const forkAgent = (source: AgentId, target: AgentId, oplogIdxCutOff: OplogIndex): void => {
  _forkCalls.push({ source, target, oplogIdxCutOff })
}
export const __getForkCalls = (): ReadonlyArray<ForkAgentCall> => [..._forkCalls]

interface RevertAgentCall {
  agentId: AgentId
  target: RevertAgentTarget
}
const _revertCalls: RevertAgentCall[] = []
export const revertAgent = (agentId: AgentId, target: RevertAgentTarget): void => {
  _revertCalls.push({ agentId, target })
}
export const __getRevertCalls = (): ReadonlyArray<RevertAgentCall> => [..._revertCalls]

let _forkResult: ForkResult = {
  tag: "original",
  val: { forkedPhantomId: { highBits: 0n, lowBits: 0n } },
}
export const fork = (): ForkResult => _forkResult
export const __setForkResult = (next: ForkResult): void => {
  _forkResult = next
}

const _componentIdsByRef = new Map<string, ComponentId>()
const _agentIdsByRef = new Map<string, AgentId>()
const _strictAgentIdsByRef = new Map<string, AgentId>()

export const resolveComponentId = (componentReference: string): ComponentId | undefined =>
  _componentIdsByRef.get(componentReference)

export const resolveAgentId = (
  componentReference: string,
  agentName: string,
): AgentId | undefined => _agentIdsByRef.get(`${componentReference}::${agentName}`)

export const resolveAgentIdStrict = (
  componentReference: string,
  agentName: string,
): AgentId | undefined => _strictAgentIdsByRef.get(`${componentReference}::${agentName}`)

export const __seedComponentId = (ref: string, id: ComponentId): void => {
  _componentIdsByRef.set(ref, id)
}
export const __seedAgentId = (ref: string, name: string, id: AgentId): void => {
  _agentIdsByRef.set(`${ref}::${name}`, id)
}
export const __seedStrictAgentId = (ref: string, name: string, id: AgentId): void => {
  _strictAgentIdsByRef.set(`${ref}::${name}`, id)
}

// ---------------------------------------------------------------------------
// GetAgents pager
// ---------------------------------------------------------------------------

const _agentsByComponent = new Map<string, ReadonlyArray<ReadonlyArray<AgentMetadata>>>()

export class GetAgents {
  private cursor = 0
  private readonly chunks: ReadonlyArray<ReadonlyArray<AgentMetadata>>

  constructor(componentId: ComponentId, _filter: AgentAnyFilter | undefined, _precise: boolean) {
    const key = uuidToString(componentId.uuid)
    this.chunks = _agentsByComponent.get(key) ?? []
  }

  getNext(): AgentMetadata[] | undefined {
    if (this.cursor >= this.chunks.length) return undefined
    const out = [...this.chunks[this.cursor]!]
    this.cursor += 1
    return out
  }
}

export const __seedAgentsForComponent = (
  componentId: ComponentId,
  chunks: ReadonlyArray<ReadonlyArray<AgentMetadata>>,
): void => {
  _agentsByComponent.set(uuidToString(componentId.uuid), chunks)
}

// ---------------------------------------------------------------------------
// Promise rendezvous
// ---------------------------------------------------------------------------

export type PromiseId = { agentId: AgentId; oplogIdx: OplogIndex }

interface PendingPromise {
  payload?: Uint8Array
  subscribers: Array<() => void>
}

const _promises = new Map<string, PendingPromise>()
const promiseKey = (id: PromiseId): string => `${agentIdKey(id.agentId)}::${id.oplogIdx.toString()}`

export const createPromise = (): PromiseId => {
  const id: PromiseId = {
    agentId: _selfMetadata.agentId,
    oplogIdx: getOplogIndex(),
  }
  _promises.set(promiseKey(id), { subscribers: [] })
  return id
}

export const completePromise = (id: PromiseId, payload: Uint8Array): boolean => {
  const entry = _promises.get(promiseKey(id))
  if (entry === undefined) return false
  if (entry.payload !== undefined) return false
  entry.payload = payload
  for (const sub of entry.subscribers.splice(0)) sub()
  return true
}

export class GetPromiseResult {
  private readonly id: PromiseId
  constructor(id: PromiseId) {
    this.id = id
  }
  subscribe(): {
    promise(): Promise<void>
    abortablePromise(signal: AbortSignal): Promise<void>
  } {
    const promise = (): Promise<void> =>
      new Promise<void>((resolve) => {
        const entry = _promises.get(promiseKey(this.id))
        if (entry === undefined || entry.payload !== undefined) {
          resolve()
          return
        }
        entry.subscribers.push(resolve)
      })
    return {
      promise,
      abortablePromise: (signal: AbortSignal) => {
        if (signal.aborted) {
          return Promise.reject(new DOMException("aborted", "AbortError"))
        }
        const entry = _promises.get(promiseKey(this.id))
        if (entry === undefined || entry.payload !== undefined) {
          return Promise.resolve()
        }
        return new Promise<void>((resolve, reject) => {
          const onAbort = (): void => {
            const idx = entry.subscribers.indexOf(onReady)
            if (idx >= 0) entry.subscribers.splice(idx, 1)
            signal.removeEventListener("abort", onAbort)
            reject(new DOMException("aborted", "AbortError"))
          }
          const onReady = (): void => {
            signal.removeEventListener("abort", onAbort)
            resolve()
          }
          entry.subscribers.push(onReady)
          signal.addEventListener("abort", onAbort, { once: true })
        })
      },
    }
  }
  get(): Uint8Array | undefined {
    return _promises.get(promiseKey(this.id))?.payload
  }
}

export const getPromise = (id: PromiseId): GetPromiseResult => new GetPromiseResult(id)

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

export const __resetAll = (): void => {
  counter = 0n
  _persistenceLevel = { tag: "smart" }
  _idempotenceMode = true
  _oplogIndex = 0n
  _atomicMarks.length = 0
  _oplogCommits.length = 0
  _selfMetadata = {
    agentId: { componentId: defaultComponentId, agentId: "Test()" },
    args: [],
    env: [],
    config: [],
    status: "running",
    componentRevision: 0n,
    retryCount: 0n,
    environmentId: defaultEnvironmentId,
  }
  _otherMetadata.clear()
  _updateCalls.length = 0
  _forkCalls.length = 0
  _revertCalls.length = 0
  _forkResult = {
    tag: "original",
    val: { forkedPhantomId: { highBits: 0n, lowBits: 0n } },
  }
  _componentIdsByRef.clear()
  _agentIdsByRef.clear()
  _strictAgentIdsByRef.clear()
  _agentsByComponent.clear()
  _promises.clear()
}

export const __reset = __resetAll
