import { Effect, Option, Stream } from "effect"
import type * as OplogHost from "golem:api/oplog@1.5.0"
import { OplogClient } from "./host/OplogClient.js"

/**
 * Effect-idiomatic façade over `golem:api/oplog@1.5.0`.
 *
 * Exposes the agent's own oplog accessors:
 *
 * - {@link currentIndex} — read the persistent log's current index
 * - {@link setIndex} — time-travel back to a previous index (advanced)
 * - {@link read} / {@link reader} — paged read of a specific agent's oplog
 * - {@link search} — paged full-text search of a specific agent's oplog
 * - {@link enrich} — resolve raw entries into public-shape entries
 *
 * Streaming variants (`read`, `search`) drive the host's pager classes
 * via `Stream.unfoldEffect` so callers can pipeline arbitrarily large
 * oplogs without buffering everything in memory.
 */

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

export type {
  AgentId,
  ComponentRevision,
  EnvironmentId,
  OplogEntry,
  OplogIndex,
  PublicOplogEntry,
} from "golem:api/oplog@1.5.0"

type RawOplogIndex = OplogHost.OplogIndex
type RawAgentId = OplogHost.AgentId
type RawEnvironmentId = OplogHost.EnvironmentId
type RawComponentRevision = OplogHost.ComponentRevision
type RawPublicOplogEntry = OplogHost.PublicOplogEntry
type RawOplogEntry = OplogHost.OplogEntry

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Raised when a `golem:api/oplog@1.5.0` (or oplog-index host call) throws. */
export class OplogHostError {
  readonly _tag = "OplogHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `OplogHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/** Read the current position in the persistent oplog. */
export const currentIndex: Effect.Effect<RawOplogIndex, OplogHostError, OplogClient> = Effect.gen(
  function* () {
    const client = yield* OplogClient
    return yield* Effect.try({
      try: () => client.getOplogIndex(),
      catch: (e) => new OplogHostError(e),
    })
  },
)

/**
 * Imperative time-travel: rewind execution to a previous oplog index.
 * Marked dangerous — most app code should not need this. Mirrors the
 * official SDK's `setOplogIndex`.
 */
export const setIndex = (idx: RawOplogIndex): Effect.Effect<void, OplogHostError, OplogClient> =>
  Effect.gen(function* () {
    const client = yield* OplogClient
    return yield* Effect.try({
      try: () => client.setOplogIndex(idx),
      catch: (e) => new OplogHostError(e),
    })
  })

/**
 * Resolve raw oplog entries into the public-shape entries by replaying
 * payload references and attaching component metadata.
 */
export const enrich = (input: {
  readonly environmentId: RawEnvironmentId
  readonly agentId: RawAgentId
  readonly entries: ReadonlyArray<readonly [RawOplogIndex, RawOplogEntry]>
  readonly componentRevision: RawComponentRevision
}): Effect.Effect<ReadonlyArray<RawPublicOplogEntry>, OplogHostError, OplogClient> =>
  Effect.gen(function* () {
    const client = yield* OplogClient
    return yield* Effect.try({
      try: () =>
        client.enrichOplogEntries(
          input.environmentId,
          input.agentId,
          input.entries.map(([i, e]) => [i, e] as [RawOplogIndex, RawOplogEntry]),
          input.componentRevision,
        ),
      catch: (e) => new OplogHostError(e),
    })
  })

// ---------------------------------------------------------------------------
// Paged read (`GetOplog`)
// ---------------------------------------------------------------------------

/**
 * Low-level handle around the host's `GetOplog` pager. Useful when
 * callers want explicit control over batching; otherwise prefer
 * {@link read}.
 */
export interface OplogReader {
  /** Fetch the next chunk of entries, or `undefined` when exhausted. */
  readonly next: Effect.Effect<ReadonlyArray<RawPublicOplogEntry> | undefined, OplogHostError>
}

/** Construct a `GetOplog` pager. */
export const reader = (input: {
  readonly agentId: RawAgentId
  readonly start: RawOplogIndex
}): Effect.Effect<OplogReader, OplogHostError, OplogClient> =>
  Effect.gen(function* () {
    const client = yield* OplogClient
    const handle = yield* Effect.try({
      try: () => client.newGetOplog(input.agentId, input.start),
      catch: (e) => new OplogHostError(e),
    })
    return {
      next: Effect.try({
        try: () => {
          const out = handle.getNext()
          return out === undefined ? undefined : ([...out] as ReadonlyArray<RawPublicOplogEntry>)
        },
        catch: (e) => new OplogHostError(e),
      }),
    }
  })

/**
 * Stream the agent's oplog starting from `start`. The host's pager
 * yields chunks until exhausted; this effect flattens them into a
 * single stream of entries.
 */
export const read = (input: {
  readonly agentId: RawAgentId
  readonly start: RawOplogIndex
}): Stream.Stream<RawPublicOplogEntry, OplogHostError, OplogClient> =>
  Stream.unwrap(
    Effect.map(reader(input), (r) =>
      Stream.paginate<OplogReader, RawPublicOplogEntry, OplogHostError>(r, (state) =>
        Effect.map(state.next, (chunk) =>
          chunk === undefined
            ? ([[], Option.none()] as const)
            : chunk.length === 0
              ? ([[], Option.some(state)] as const)
              : ([Array.from(chunk), Option.some(state)] as const),
        ),
      ),
    ),
  )

// ---------------------------------------------------------------------------
// Paged search (`SearchOplog`)
// ---------------------------------------------------------------------------

/** Low-level handle around the host's `SearchOplog` pager. */
export interface OplogSearchReader {
  readonly next: Effect.Effect<
    ReadonlyArray<readonly [RawOplogIndex, RawPublicOplogEntry]> | undefined,
    OplogHostError
  >
}

/** Construct a `SearchOplog` pager. */
export const searchReader = (input: {
  readonly agentId: RawAgentId
  readonly text: string
}): Effect.Effect<OplogSearchReader, OplogHostError, OplogClient> =>
  Effect.gen(function* () {
    const client = yield* OplogClient
    const handle = yield* Effect.try({
      try: () => client.newSearchOplog(input.agentId, input.text),
      catch: (e) => new OplogHostError(e),
    })
    return {
      next: Effect.try({
        try: () => {
          const out = handle.getNext()
          return out === undefined
            ? undefined
            : (out.map(([i, e]) => [i, e] as [RawOplogIndex, RawPublicOplogEntry]) as ReadonlyArray<
                readonly [RawOplogIndex, RawPublicOplogEntry]
              >)
        },
        catch: (e) => new OplogHostError(e),
      }),
    }
  })

/**
 * Stream the host's full-text search results over the agent's oplog,
 * yielding `(index, entry)` tuples.
 */
export const search = (input: {
  readonly agentId: RawAgentId
  readonly text: string
}): Stream.Stream<readonly [RawOplogIndex, RawPublicOplogEntry], OplogHostError, OplogClient> =>
  Stream.unwrap(
    Effect.map(searchReader(input), (r) =>
      Stream.paginate<
        OplogSearchReader,
        readonly [RawOplogIndex, RawPublicOplogEntry],
        OplogHostError
      >(r, (state) =>
        Effect.map(state.next, (chunk) =>
          chunk === undefined
            ? ([[], Option.none()] as const)
            : chunk.length === 0
              ? ([[], Option.some(state)] as const)
              : ([Array.from(chunk), Option.some(state)] as const),
        ),
      ),
    ),
  )
