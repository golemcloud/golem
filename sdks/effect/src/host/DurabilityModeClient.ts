/**
 * Host service for the execution-mode subset of
 * `golem:api/host@1.5.0`:
 *
 * - `oplogCommit` (replication barrier)
 * - `generateIdempotencyKey` (persisted UUID generator used by RPC)
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"

export interface DurabilityModeClientShape {
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
    oplogCommit: (replicas) => ApiHost.oplogCommit(replicas),
    generateIdempotencyKey: () => ApiHost.generateIdempotencyKey(),
  }),
)
