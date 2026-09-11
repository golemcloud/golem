/**
 * Host service for the execution-mode subset of
 * `golem:api/host@1.5.0`:
 *
 * - persistence level (`getOplogPersistenceLevel` /
 *   `setOplogPersistenceLevel`)
 * - idempotence mode (`getIdempotenceMode` / `setIdempotenceMode`)
 * - atomic regions (`markBeginOperation` / `markEndOperation`)
 * - `oplogCommit` (replication barrier)
 * - `generateIdempotencyKey` (oplog-bypass UUID generator) — bundled
 *   here per plan §4a; downstream consumers (e.g. `src/client.ts` for
 *   RPC) will read it from this client once their refactor lands.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"

type RawPersistenceLevel = ApiHost.PersistenceLevel
type RawOplogIndex = ApiHost.OplogIndex

export interface DurabilityModeClientShape {
  /** Mirrors `golem:api/host.getOplogPersistenceLevel`. */
  readonly getOplogPersistenceLevel: () => RawPersistenceLevel
  /** Mirrors `golem:api/host.setOplogPersistenceLevel`. */
  readonly setOplogPersistenceLevel: (next: RawPersistenceLevel) => void
  /** Mirrors `golem:api/host.getIdempotenceMode`. */
  readonly getIdempotenceMode: () => boolean
  /** Mirrors `golem:api/host.setIdempotenceMode`. */
  readonly setIdempotenceMode: (next: boolean) => void
  /** Mirrors `golem:api/host.markBeginOperation`. */
  readonly markBeginOperation: () => RawOplogIndex
  /** Mirrors `golem:api/host.markEndOperation`. */
  readonly markEndOperation: (begin: RawOplogIndex) => void
  /** Mirrors `golem:api/host.oplogCommit`. */
  readonly oplogCommit: (replicas: number) => void
  /** Mirrors `golem:api/host.generateIdempotencyKey`. */
  readonly generateIdempotencyKey: () => ApiHost.Uuid
}

export class DurabilityModeClient extends Context.Service<
  DurabilityModeClient,
  DurabilityModeClientShape
>()("effect-golem/host/DurabilityMode") {}

export const DurabilityModeLive: Layer.Layer<DurabilityModeClient> = Layer.succeed(
  DurabilityModeClient,
  DurabilityModeClient.of({
    getOplogPersistenceLevel: () => ApiHost.getOplogPersistenceLevel(),
    setOplogPersistenceLevel: (next) => ApiHost.setOplogPersistenceLevel(next),
    getIdempotenceMode: () => ApiHost.getIdempotenceMode(),
    setIdempotenceMode: (next) => ApiHost.setIdempotenceMode(next),
    markBeginOperation: () => ApiHost.markBeginOperation(),
    markEndOperation: (begin) => ApiHost.markEndOperation(begin),
    oplogCommit: (replicas) => ApiHost.oplogCommit(replicas),
    generateIdempotencyKey: () => ApiHost.generateIdempotencyKey(),
  }),
)
