/**
 * Runtime mock for `golem:api/oplog@1.5.0`. Implements only the
 * methods/classes the SDK actually uses:
 *
 * - `GetOplog` — paged read, seeded via `__seedReadChunks`
 * - `SearchOplog` — paged search, seeded via `__seedSearchChunks`
 * - `enrichOplogEntries` — delegated to a seeded function (default:
 *   identity-pass-through that throws "not seeded")
 *
 * State is in-memory and reset via `__reset()`.
 */

import type { AgentId, OplogIndex } from "./golem-core-types.js"
import { uuidToString } from "./golem-core-types.js"

// ---------------------------------------------------------------------------
// Minimal subset of the public oplog entry types
// ---------------------------------------------------------------------------

/**
 * Test-mock view of the host's `AttributedPublicOplogEntry`. Only the
 * tags of the attribution and of the entry are inspected by the tests.
 */
export type AttributedPublicOplogEntry = {
  attribution: { tag: string } & Record<string, unknown>
  entry: { tag: string } & Record<string, unknown>
}

export type OplogEntry = { tag: string } & Record<string, unknown>

// ---------------------------------------------------------------------------
// Seeded state
// ---------------------------------------------------------------------------

const _readChunks = new Map<string, ReadonlyArray<ReadonlyArray<AttributedPublicOplogEntry>>>()
const _searchChunks = new Map<
  string,
  ReadonlyArray<ReadonlyArray<readonly [OplogIndex, AttributedPublicOplogEntry]>>
>()
let _enrichImpl:
  | ((
      entries: ReadonlyArray<readonly [OplogIndex, OplogEntry]>,
    ) => ReadonlyArray<AttributedPublicOplogEntry>)
  | null = null

const readKey = (id: AgentId, start: OplogIndex): string =>
  `${uuidToString(id.uuid)}::${start.toString()}`
const searchKey = (id: AgentId, text: string): string => `${uuidToString(id.uuid)}::${text}`

export const __seedReadChunks = (
  agentId: AgentId,
  start: OplogIndex,
  chunks: ReadonlyArray<ReadonlyArray<AttributedPublicOplogEntry>>,
): void => {
  _readChunks.set(readKey(agentId, start), chunks)
}

export const __seedSearchChunks = (
  agentId: AgentId,
  text: string,
  chunks: ReadonlyArray<ReadonlyArray<readonly [OplogIndex, AttributedPublicOplogEntry]>>,
): void => {
  _searchChunks.set(searchKey(agentId, text), chunks)
}

export const __setEnrichImpl = (
  fn: (
    entries: ReadonlyArray<readonly [OplogIndex, OplogEntry]>,
  ) => ReadonlyArray<AttributedPublicOplogEntry>,
): void => {
  _enrichImpl = fn
}

// ---------------------------------------------------------------------------
// `enrichOplogEntries`
// ---------------------------------------------------------------------------

export type EnvironmentId = { uuid: { highBits: bigint; lowBits: bigint } }
export type ComponentRevision = bigint

export const enrichOplogEntries = (
  _environmentId: EnvironmentId,
  _agentId: AgentId,
  entries: ReadonlyArray<readonly [OplogIndex, OplogEntry]>,
  _componentRevision: ComponentRevision,
): AttributedPublicOplogEntry[] => {
  if (_enrichImpl === null) {
    throw new Error("test mock: enrichOplogEntries was called but no impl is seeded")
  }
  return [..._enrichImpl(entries)]
}

// ---------------------------------------------------------------------------
// `GetOplog` pager
// ---------------------------------------------------------------------------

export class GetOplog {
  private cursor = 0
  private readonly chunks: ReadonlyArray<ReadonlyArray<AttributedPublicOplogEntry>>

  constructor(agentId: AgentId, start: OplogIndex) {
    this.chunks = _readChunks.get(readKey(agentId, start)) ?? []
  }

  getNext(): AttributedPublicOplogEntry[] | undefined {
    if (this.cursor >= this.chunks.length) return undefined
    const out = [...this.chunks[this.cursor]!]
    this.cursor += 1
    return out
  }
}

// ---------------------------------------------------------------------------
// `SearchOplog` pager
// ---------------------------------------------------------------------------

export class SearchOplog {
  private cursor = 0
  private readonly chunks: ReadonlyArray<
    ReadonlyArray<readonly [OplogIndex, AttributedPublicOplogEntry]>
  >

  constructor(agentId: AgentId, text: string) {
    this.chunks = _searchChunks.get(searchKey(agentId, text)) ?? []
  }

  getNext(): Array<[OplogIndex, AttributedPublicOplogEntry]> | undefined {
    if (this.cursor >= this.chunks.length) return undefined
    const out = this.chunks[this.cursor]!.map(
      ([i, e]) => [i, e] as [OplogIndex, AttributedPublicOplogEntry],
    )
    this.cursor += 1
    return out
  }
}

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

export const __reset = (): void => {
  _readChunks.clear()
  _searchChunks.clear()
  _enrichImpl = null
}
