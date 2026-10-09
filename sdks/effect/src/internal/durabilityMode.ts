/**
 * Effect-idiomatic façade over the execution-mode controls on
 * `golem:api/host@1.5.0`:
 *
 * - idempotence mode (get / set / scoped)
 * - atomic regions via `mark-begin-operation` / `mark-end-operation`
 * - explicit `oplog-commit` (replication barrier)
 * - `generate-idempotency-key` (oplog-bypass UUID generator)
 *
 * **Example**
 *
 * ```ts
 * import { Durability } from "@golemcloud/effect-golem"
 *
 * yield* Durability.atomically(doWork)
 * yield* Durability.oplogCommit(2)
 * ```
 *
 * Every call is wrapped in `Effect.try` and surfaces unexpected host
 * throws as {@link DurabilityHostError}; user-supplied numeric inputs
 * are validated with {@link DurabilityValidationError}.
 *
 * @since 1.5.0
 */
import { Cause, Effect, Scope } from "effect"
import type * as ApiHost from "golem:api/host@1.5.0"
import { AgentHostClient } from "../host/AgentHostClient.js"
import { DurabilityModeClient } from "../host/DurabilityModeClient.js"
import { currentIndex, setIndex, type OplogHostError } from "../Oplog.js"
import { OplogClient } from "../host/OplogClient.js"

// ---------------------------------------------------------------------------
// trap reason formatter
// ---------------------------------------------------------------------------

/**
 * Format an `Effect` failure (typed value, defect, or interruption) as
 * the `reason` argument to {@link AgentHostClient.trap}. Mirrors the
 * upstream `formatErrorForTrap` helper in
 * `sdks/ts/packages/golem-ts-sdk/src/host/guard.ts`: prefer a stack,
 * fall back to `name: message`, then `String(...)`.
 *
 * @internal
 */
export const formatCauseForTrap = <E>(cause: Cause.Cause<E>): string => {
  for (const reason of cause.reasons) {
    if (Cause.isFailReason(reason)) {
      const e = reason.error as unknown
      if (e instanceof Error) return e.stack ?? `${e.name}: ${e.message}`
      try {
        return String(e)
      } catch {
        return "<unprintable error>"
      }
    }
    if (Cause.isDieReason(reason)) {
      const e = reason.defect as unknown
      if (e instanceof Error) return e.stack ?? `${e.name}: ${e.message}`
      try {
        return String(e)
      } catch {
        return "<unprintable defect>"
      }
    }
    if (Cause.isInterruptReason(reason)) return "interrupted"
  }
  return Cause.pretty(cause)
}

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

/**
 * @since 1.5.0
 * @category re-exports
 */
export type { OplogIndex, Uuid } from "golem:api/host@1.5.0"

type RawOplogIndex = ApiHost.OplogIndex

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

// Branded so the durable-function wrapper can route this into the
// defect channel via nominal detection. See
// `src/durable-function.ts` (`sdkErrorBrand`) for the rationale.
// TypeScript requires class-property computed names to have a `unique
// symbol` type, so we declare the brand as a `unique symbol` const
// here; `Symbol.for(...)` guarantees we get the same runtime symbol
// across modules.
const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/**
 * Raised when a `golem:api/host@1.5.0` execution-mode call throws unexpectedly.
 *
 * @since 1.5.0
 * @category errors
 */
export class DurabilityHostError {
  readonly _tag = "DurabilityHostError"
  readonly [sdkErrorBrand] = true
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `DurabilityHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/**
 * Raised when a user-supplied input to a durability call is out of range.
 *
 * @since 1.5.0
 * @category errors
 */
export class DurabilityValidationError {
  readonly _tag = "DurabilityValidationError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `DurabilityValidationError: ${reason}`
  }
}

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/**
 * Read the current idempotence mode. `true` = at-least-once; `false` = at-most-once.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const getIdempotenceMode: Effect.Effect<boolean, DurabilityHostError, DurabilityModeClient> =
  Effect.gen(function* () {
    const client = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => client.getIdempotenceMode(),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Write the idempotence mode.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const setIdempotenceMode = (
  idempotent: boolean,
): Effect.Effect<void, DurabilityHostError, DurabilityModeClient> =>
  Effect.gen(function* () {
    const client = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => client.setIdempotenceMode(idempotent),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Block until the oplog has been written to at least `replicas`
 * replicas (or the maximum if `replicas` exceeds the maximum). The host
 * type is `u8`, so `replicas` is validated to fit `0..=255`.
 *
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
 * Mark the beginning of an atomic region. Returns the host's chosen
 * `OplogIndex` to be passed back into {@link endOperation}. Prefer
 * {@link atomically} unless you really need imperative control.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const beginOperation: Effect.Effect<
  RawOplogIndex,
  DurabilityHostError,
  DurabilityModeClient
> = Effect.gen(function* () {
  const client = yield* DurabilityModeClient
  return yield* Effect.try({
    try: () => client.markBeginOperation(),
    catch: (e) => new DurabilityHostError(e),
  })
})

/**
 * Mark the end of the atomic region whose begin index is `begin`.
 * Idempotent on the host side: subsequent calls with the same index
 * are no-ops.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const endOperation = (
  begin: RawOplogIndex,
): Effect.Effect<void, DurabilityHostError, DurabilityModeClient> =>
  Effect.gen(function* () {
    const client = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => client.markEndOperation(begin),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Generate a fresh idempotency key. This call is committed to the
 * oplog so the same key is returned on replay — making the value safe
 * to use against external systems' idempotence checks (e.g. payment
 * gateways).
 *
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

// ---------------------------------------------------------------------------
// Scoped activation
// ---------------------------------------------------------------------------

/**
 * Acquire-release pair that sets the idempotence mode on entry and
 * restores the previous value on scope close.
 *
 * @since 1.5.0
 * @category combinators
 */
export const useIdempotenceModeScoped = (
  idempotent: boolean,
): Effect.Effect<void, DurabilityHostError, Scope.Scope | DurabilityModeClient> =>
  Effect.gen(function* () {
    const previous = yield* getIdempotenceMode
    yield* Effect.acquireRelease(setIdempotenceMode(idempotent), () =>
      setIdempotenceMode(previous).pipe(Effect.ignore),
    )
  })

/**
 * Run `effect` with `idempotent` temporarily installed as the idempotence mode.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withIdempotenceMode = <A, E, R>(
  idempotent: boolean,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | DurabilityHostError, Exclude<R, Scope.Scope> | DurabilityModeClient> =>
  Effect.scoped(useIdempotenceModeScoped(idempotent).pipe(Effect.andThen(effect)))

/**
 * Skips `mark-end-operation` on a non-Success exit, leaving the
 * atomic region open so the host's replay-time recovery rolls back
 * the partial execution and re-runs the block from the begin marker.
 * Mirrors the Rust `AtomicOperationGuard::drop` impl which guards
 * `mark_end_operation` with `!std::thread::panicking()`.
 *
 * Used internally by {@link atomically}; not re-exported.
 *
 * @internal
 */
const markAtomicOperationScopedTrapping: Effect.Effect<
  RawOplogIndex,
  DurabilityHostError,
  Scope.Scope | DurabilityModeClient
> = Effect.acquireRelease(beginOperation, (begin, exit) =>
  exit._tag === "Success" ? endOperation(begin).pipe(Effect.ignore) : Effect.void,
)

/**
 * Run `effect` inside an atomic region.
 *
 * On success, the atomic region is committed via
 * `mark-end-operation`. On **any** failure (typed `E`, defect via
 * `Effect.die`, or interruption) the SDK calls
 * `golem:api/host.trap(...)` — an uncatchable wasm trap — so user
 * code outside `atomically` cannot observe the failure with
 * `Effect.catchAll` / `Effect.either` and silently continue with an
 * open atomic region. The atomic region is deliberately left open;
 * the host's replay-time recovery rolls back the partial execution
 * and retries the block from the begin marker.
 *
 * Mirrors `golem-ts-sdk@1.5.x` (`atomically` in
 * `host/guard.ts`) and the Rust `atomically_result` helper.
 *
 * @since 1.5.0
 * @category combinators
 */
export const atomically = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  E | DurabilityHostError,
  Exclude<R, Scope.Scope> | DurabilityModeClient | AgentHostClient
> =>
  Effect.scoped(
    markAtomicOperationScopedTrapping.pipe(
      Effect.andThen(
        Effect.catchCause(effect, (cause) =>
          Effect.gen(function* () {
            const host = yield* AgentHostClient
            // Calling `trap` is uncatchable in production (wasm trap).
            // In test mocks the call throws a `TestTrapError`; either
            // path leaves the surrounding scope to close with a
            // non-Success exit, so `mark-end-operation` is skipped.
            yield* Effect.sync(() => host.trap(`atomic block failed: ${formatCauseForTrap(cause)}`))
            return yield* Effect.failCause(cause)
          }),
        ),
      ),
    ),
  )

// ---------------------------------------------------------------------------
// unwrapOrRevert / checkpoint / compensable
// ---------------------------------------------------------------------------

/**
 * Rewind within the invocation. Successful host control transfer never returns;
 * a returning host is a defect, not a suspended fiber or a successful rollback.
 */
const rollback = (
  checkpointIdx: ApiHost.OplogIndex,
): Effect.Effect<never, OplogHostError, OplogClient> =>
  setIndex(checkpointIdx).pipe(
    Effect.andThen(Effect.die(new Error("Unreachable: reverted to checkpoint"))),
  )

/**
 * Capture a checkpoint and run an Effect, rewinding within the current
 * invocation on typed failure. Defects and interruption propagate unchanged.
 * Rollback never returns a value. External side effects are not undone.
 *
 * @since 1.5.0
 * @category combinators
 */
export const unwrapOrRevert = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, OplogHostError, R | OplogClient> =>
  Effect.gen(function* () {
    const cp = yield* checkpoint
    return yield* cp.runOrRevert(effect)
  })

/**
 * An invocation-local checkpoint. Rollback restarts execution at its
 * captured oplog index, rather than issuing a management revert.
 *
 * @since 1.5.0
 * @category models
 */
export class Checkpoint {
  constructor(private readonly index: ApiHost.OplogIndex) {}

  /**
   * Rewind to the captured index. Successful rollback never returns.
   * @since 1.5.0
   * @category operations
   */
  get revert(): Effect.Effect<never, OplogHostError, OplogClient> {
    return rollback(this.index)
  }

  /**
   * Return success or rewind on typed failure. Defects and interruption propagate.
   * @since 1.5.0
   * @category combinators
   */
  runOrRevert<A, E, R>(
    effect: Effect.Effect<A, E, R>,
  ): Effect.Effect<A, OplogHostError, R | OplogClient> {
    return Effect.catchCause(effect, (cause) =>
      cause.reasons.every(Cause.isFailReason)
        ? this.revert
        : Effect.failCause(cause as Cause.Cause<never>),
    )
  }

  /**
   * Rewind when the condition is false.
   * @since 1.5.0
   * @category operations
   */
  assertOrRevert(condition: boolean): Effect.Effect<void, OplogHostError, OplogClient> {
    return condition ? Effect.void : this.revert
  }
}

/**
 * Capture the current oplog index when this Effect runs. Use the checkpoint
 * only within the invocation that created it; do not store it in agent state.
 *
 * @since 1.5.0
 * @category constructors
 */
export const checkpoint: Effect.Effect<Checkpoint, OplogHostError, OplogClient> = Effect.map(
  currentIndex,
  (index) => new Checkpoint(index),
)

/**
 * Saga-flavoured combinator: acquire a value, run a body with it, and
 * register a `compensate` Effect that runs only on body failure
 * (before checkpoint rollback). The compensator is intended for external
 * side effects that the durable revert cannot undo (e.g. an HTTP call
 * to a remote system). `acquire` failures bubble up directly without
 * compensation; defects and interruption bubble up unchanged. Typed compensation
 * failures are ignored; compensation defects prevent rollback.
 *
 * @since 1.5.0
 * @category combinators
 */
export const compensable = <A, B, E, R>(input: {
  readonly acquire: Effect.Effect<A, E, R>
  readonly body: (a: A) => Effect.Effect<B, E, R>
  readonly compensate: (a: A) => Effect.Effect<void, unknown, R>
}): Effect.Effect<B, E | OplogHostError, R | OplogClient> =>
  Effect.gen(function* () {
    const checkpointIdx = yield* currentIndex
    const a = yield* input.acquire
    return yield* Effect.catchCause(input.body(a), (cause) => {
      if (!cause.reasons.every(Cause.isFailReason)) {
        return Effect.failCause(cause as Cause.Cause<never>)
      }
      return input.compensate(a).pipe(
        Effect.catchCause((cause) =>
          cause.reasons.every(Cause.isFailReason)
            ? Effect.void
            : Effect.failCause(cause as Cause.Cause<never>),
        ),
        Effect.andThen(rollback(checkpointIdx)),
      )
    })
  })
