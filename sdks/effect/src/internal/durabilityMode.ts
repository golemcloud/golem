/** Execution controls and idempotency keys. @since 1.5.0 */
import { Cause, Effect, Predicate } from "effect"
import type * as ApiHost from "golem:api/host@1.5.0"
import { DurabilityModeClient } from "../host/DurabilityModeClient.js"

/** Format a failure for the host trap. @internal */
export const formatCauseForTrap = <E>(cause: Cause.Cause<E>): string => {
  for (const reason of cause.reasons) {
    if (Cause.isInterruptReason(reason)) return "interrupted"
    const e = Cause.isFailReason(reason) ? reason.error : reason.defect
    if (e instanceof Error) return e.stack ?? `${e.name}: ${e.message}`
    try {
      if (Predicate.hasProperty(e, "message") && Predicate.isString(e.message)) return e.message
      return String(e)
    } catch {
      return Cause.isFailReason(reason) ? "<unprintable error>" : "<unprintable defect>"
    }
  }
  return Cause.pretty(cause)
}

/** @since 1.5.0 @category re-exports */
export type { OplogIndex, Uuid } from "golem:api/host@1.5.0"

const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/** Unexpected host failure. @since 1.5.0 @category errors */
export class DurabilityHostError {
  readonly _tag = "DurabilityHostError"
  readonly [sdkErrorBrand] = true
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `DurabilityHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/** Invalid durability input. @since 1.5.0 @category errors */
export class DurabilityValidationError {
  readonly _tag = "DurabilityValidationError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `DurabilityValidationError: ${reason}`
  }
}

/**
 * Wait for oplog replication to the requested number of replicas.
 * @since 1.5.0
 * @category host bindings
 */
export const oplogCommit = (
  replicas: number,
): Effect.Effect<void, DurabilityHostError | DurabilityValidationError, DurabilityModeClient> => {
  if (!Number.isSafeInteger(replicas) || replicas < 0 || replicas > 0xff) {
    return Effect.fail(
      new DurabilityValidationError(
        `oplogCommit.replicas must be an integer in 0..=255 (got ${String(replicas)})`,
      ),
    )
  }
  return Effect.gen(function* () {
    const client = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => client.oplogCommit(replicas),
      catch: (e) => new DurabilityHostError(e),
    })
  })
}

/**
 * Generate a fresh idempotency key, recorded for replay.
 * @since 1.5.0
 * @category host bindings
 */
export const generateIdempotencyKey: Effect.Effect<
  ApiHost.Uuid,
  DurabilityHostError,
  DurabilityModeClient
> = Effect.gen(function* () {
  const client = yield* DurabilityModeClient
  return yield* Effect.try({
    try: () => client.generateIdempotencyKey(),
    catch: (e) => new DurabilityHostError(e),
  })
})
