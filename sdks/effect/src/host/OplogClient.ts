/**
 * Host service bundling the agent's oplog accessors:
 *
 * - `golem:api/host@1.5.0` — `getOplogIndex` / `setOplogIndex` (read /
 *   time-travel the agent's own persistent oplog cursor).
 * - `golem:api/oplog@1.5.0` — `enrichOplogEntries` plus the
 *   `GetOplog` and `SearchOplog` resource constructors used to drive
 *   paged reads of an agent's oplog.
 *
 * The cursor pagers are exposed as scoped `Effect`s — the host's
 * `GetOplog` / `SearchOplog` resources are dropped when the surrounding
 * `Scope` closes (the WIT contract says "no-op on already-dropped"; we
 * rely on JS GC since `wasi:io`-style explicit `[resource-drop]` is
 * not exposed for these classes), so consumers do not have to remember
 * to release the handle manually.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"
import * as OplogHost from "golem:api/oplog@1.5.0"

type RawOplogIndex = OplogHost.OplogIndex
type RawAgentId = OplogHost.AgentId
type RawEnvironmentId = OplogHost.EnvironmentId
type RawComponentRevision = OplogHost.ComponentRevision
type RawPublicOplogEntry = OplogHost.PublicOplogEntry
type RawOplogEntry = OplogHost.OplogEntry

/**
 * Pager handle around `golem:api/oplog.GetOplog`. Calls
 * `getNext()` synchronously; returns `undefined` once the host's
 * underlying iterator is exhausted.
 */
export interface GetOplogHandle {
  readonly getNext: () => ReadonlyArray<RawPublicOplogEntry> | undefined
}

/** Pager handle around `golem:api/oplog.SearchOplog`. */
export interface SearchOplogHandle {
  readonly getNext: () => ReadonlyArray<readonly [RawOplogIndex, RawPublicOplogEntry]> | undefined
}

export interface OplogClientShape {
  /** Mirrors `golem:api/host.getOplogIndex`. */
  readonly getOplogIndex: () => RawOplogIndex
  /** Mirrors `golem:api/host.setOplogIndex`. */
  readonly setOplogIndex: (idx: RawOplogIndex) => void
  /** Mirrors `golem:api/oplog.enrichOplogEntries`. */
  readonly enrichOplogEntries: (
    environmentId: RawEnvironmentId,
    agentId: RawAgentId,
    entries: ReadonlyArray<readonly [RawOplogIndex, RawOplogEntry]>,
    componentRevision: RawComponentRevision,
  ) => ReadonlyArray<RawPublicOplogEntry>
  /**
   * Construct a `GetOplog` pager. The handle is a thin wrapper over
   * the host's resource so its `getNext()` matches the WIT shape;
   * production resolution allocates a real host resource.
   */
  readonly newGetOplog: (agentId: RawAgentId, start: RawOplogIndex) => GetOplogHandle
  /** Construct a `SearchOplog` pager. */
  readonly newSearchOplog: (agentId: RawAgentId, text: string) => SearchOplogHandle
}

export class OplogClient extends Context.Service<OplogClient, OplogClientShape>()(
  "effect-golem/host/Oplog",
) {}

export const OplogLive: Layer.Layer<OplogClient> = Layer.succeed(
  OplogClient,
  OplogClient.of({
    getOplogIndex: () => ApiHost.getOplogIndex(),
    setOplogIndex: (idx) => ApiHost.setOplogIndex(idx),
    enrichOplogEntries: (environmentId, agentId, entries, componentRevision) =>
      OplogHost.enrichOplogEntries(
        environmentId,
        agentId,
        entries.map(([i, e]) => [i, e] as [RawOplogIndex, RawOplogEntry]),
        componentRevision,
      ),
    newGetOplog: (agentId, start) => {
      const handle = new OplogHost.GetOplog(agentId, start)
      return {
        getNext: () => {
          const out = handle.getNext()
          return out === undefined ? undefined : ([...out] as ReadonlyArray<RawPublicOplogEntry>)
        },
      }
    },
    newSearchOplog: (agentId, text) => {
      const handle = new OplogHost.SearchOplog(agentId, text)
      return {
        getNext: () => {
          const out = handle.getNext()
          return out === undefined
            ? undefined
            : (out.map(([i, e]) => [i, e] as [RawOplogIndex, RawPublicOplogEntry]) as ReadonlyArray<
                readonly [RawOplogIndex, RawPublicOplogEntry]
              >)
        },
      }
    },
  }),
)
