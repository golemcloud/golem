/**
 * Effect-idiomatic façade over the execution-mode controls on
 * `golem:api/host@1.5.0`:
 *
 * - persistence level (get / set / scoped)
 * - idempotence mode (get / set / scoped)
 * - atomic regions via `mark-begin-operation` / `mark-end-operation`
 * - explicit `oplog-commit` (replication barrier)
 * - `generate-idempotency-key` (oplog-bypass UUID generator)
 *
 * **Example**
 *
 * ```ts
 * import { Durability } from "effect-golem"
 *
 * yield* Durability.atomically(doWork)
 * yield* Durability.withPersistenceLevel(Durability.PersistenceLevel.persistNothing, doWork)
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
import { revertAgent, RevertTarget, type AgentsHostError } from "../Agents.js"
import { AgentHostClient } from "../host/AgentHostClient.js"
import { DurabilityModeClient } from "../host/DurabilityModeClient.js"
import { currentIndex, type OplogHostError } from "../Oplog.js"
import { OplogClient } from "../host/OplogClient.js"
import { SelfAgentId } from "../SelfAgentId.js"

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

/**
 * Re-exported from `golem:api/host@1.5.0` as a type alias so the
 * value-level {@link PersistenceLevel} const namespace can keep the
 * un-suffixed name.
 *
 * @since 1.5.0
 * @category models
 */
export type PersistenceLevelValue = ApiHost.PersistenceLevel

type RawPersistenceLevel = ApiHost.PersistenceLevel
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
// Persistence level constructors (pure data)
// ---------------------------------------------------------------------------

/**
 * Pure-data constructors for the WIT `persistence-level` variant.
 *
 * @since 1.5.0
 * @category constructors
 */
export const PersistenceLevel = {
  /** Persist nothing (treat the body as ephemeral, no replay/restore guarantees). */
  persistNothing: { tag: "persist-nothing" } as const,
  /** Persist remote side effects only (the default before "smart"). */
  persistRemoteSideEffects: { tag: "persist-remote-side-effects" } as const,
  /** Smart auto-detection (host default; recommended for most agents). */
  smart: { tag: "smart" } as const,
} as const

/**
 * Local WIT-drift exhaustiveness witness for {@link PersistenceLevel}:
 * every tag in `golem:api/host@1.5.0.persistence-level` must have a
 * corresponding constructor here. If `golem-types/*.d.ts` is regenerated
 * with a new variant, this `satisfies` clause fails to compile and points
 * directly at the wrapper that needs updating.
 */
void ({
  "persist-nothing": PersistenceLevel.persistNothing,
  "persist-remote-side-effects": PersistenceLevel.persistRemoteSideEffects,
  smart: PersistenceLevel.smart,
} satisfies Record<RawPersistenceLevel["tag"], unknown>)

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/**
 * Read the current persistence level.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const getPersistenceLevel: Effect.Effect<
  RawPersistenceLevel,
  DurabilityHostError,
  DurabilityModeClient
> = Effect.gen(function* () {
  const client = yield* DurabilityModeClient
  return yield* Effect.try({
    try: () => client.getOplogPersistenceLevel(),
    catch: (e) => new DurabilityHostError(e),
  })
})

/**
 * Write the persistence level. Persists to the oplog.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const setPersistenceLevel = (
  level: RawPersistenceLevel,
): Effect.Effect<void, DurabilityHostError, DurabilityModeClient> =>
  Effect.gen(function* () {
    const client = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => client.setOplogPersistenceLevel(level),
      catch: (e) => new DurabilityHostError(e),
    })
  })

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
 * Acquire-release pair that sets the persistence level on entry and
 * restores the previous value on scope close (success, failure or
 * interruption).
 *
 * @since 1.5.0
 * @category combinators
 */
export const usePersistenceLevelScoped = (
  level: RawPersistenceLevel,
): Effect.Effect<void, DurabilityHostError, Scope.Scope | DurabilityModeClient> =>
  Effect.gen(function* () {
    const previous = yield* getPersistenceLevel
    yield* Effect.acquireRelease(setPersistenceLevel(level), () =>
      setPersistenceLevel(previous).pipe(Effect.ignore),
    )
  })

/**
 * Run `effect` with `level` temporarily installed as the persistence
 * level. Restores the previous level on success, error and
 * interruption.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withPersistenceLevel = <A, E, R>(
  level: RawPersistenceLevel,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | DurabilityHostError, Exclude<R, Scope.Scope> | DurabilityModeClient> =>
  Effect.scoped(usePersistenceLevelScoped(level).pipe(Effect.andThen(effect)))

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
 * Decide whether a {@link Cause.Cause} should trigger a self-revert.
 * Defects (`Die`) surface programmer bugs and should NOT silently
 * preempt the agent; only typed failures and interruption do.
 */
const shouldRevertCause = <E>(cause: Cause.Cause<E>): boolean => {
  for (const reason of cause.reasons) {
    if (Cause.isFailReason(reason) || Cause.isInterruptReason(reason)) return true
  }
  return false
}

/**
 * Issue a self-revert to the captured oplog checkpoint and then
 * suspend forever. The host is expected to preempt and restart the
 * fiber; `Effect.never` keeps the fiber alive until that preemption
 * lands.
 */
const revertAndSuspend = (
  self: ApiHost.AgentId,
  checkpointIdx: ApiHost.OplogIndex,
): Effect.Effect<never, AgentsHostError, AgentHostClient> =>
  revertAgent(self, RevertTarget.toOplogIndex(checkpointIdx)).pipe(Effect.andThen(Effect.never))

/**
 * Run a fallible Effect; if it fails (typed failure) or is
 * interrupted, revert this agent's oplog to the index captured before
 * the body ran. The host preempts and restarts the agent from the
 * checkpoint, so any durable side effects performed inside `effect`
 * "never happened" from the post-revert perspective.
 *
 * **Details**
 *
 * Defects (`Effect.die` / unexpected throws) are **not** routed to
 * revert — they propagate as defects, matching the spirit of the
 * official SDKs' `unwrap-or-revert`.
 *
 * Mirrors `golem-ts-sdk` / `golem-rust-sdk`. The returned Effect's
 * typed failures are: oplog/agent host bookkeeping errors only — the
 * body's `E` channel is suppressed because the failure branch never
 * resumes (revert is followed by `Effect.never`).
 *
 * @since 1.5.0
 * @category combinators
 */
export const unwrapOrRevert = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  OplogHostError | AgentsHostError,
  R | SelfAgentId | OplogClient | AgentHostClient
> =>
  Effect.gen(function* () {
    const checkpointIdx = yield* currentIndex
    const self = yield* SelfAgentId
    return yield* effect.pipe(
      Effect.catchCause((cause) =>
        shouldRevertCause(cause)
          ? revertAndSuspend(self, checkpointIdx)
          : (Effect.failCause(cause) as Effect.Effect<never, never, never>),
      ),
    ) as Effect.Effect<A, never, R>
  })

/**
 * Tagged result of {@link checkpoint}. The full `Cause.Cause<E>` is
 * preserved on the `reverted` branch — this honestly reflects that
 * the body may have been interrupted (no `E` value) or failed with a
 * typed error.
 *
 * @since 1.5.0
 * @category models
 */
export type CheckpointResult<A, E> =
  | { readonly _tag: "ok"; readonly value: A }
  | { readonly _tag: "reverted"; readonly cause: Cause.Cause<E> }

/**
 * Run `effect`; on success return `{ _tag: "ok", value }`. On typed
 * failure or interruption, issue a self-revert to the captured oplog
 * index AND return `{ _tag: "reverted", cause }`. Defects are
 * propagated as defects (no revert).
 *
 * **Details**
 *
 * In real Golem runtimes the `revertAgent` call typically preempts
 * before the tagged result is observed; this combinator is most
 * useful inside test runtimes that do not preempt on revert and for
 * callers that want to inspect the post-revert state.
 *
 * @since 1.5.0
 * @category combinators
 */
export const checkpoint = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<
  CheckpointResult<A, E>,
  OplogHostError | AgentsHostError,
  R | SelfAgentId | OplogClient | AgentHostClient
> =>
  Effect.gen(function* () {
    const checkpointIdx = yield* currentIndex
    const self = yield* SelfAgentId
    const exit = yield* Effect.exit(effect)
    if (exit._tag === "Success") {
      return { _tag: "ok", value: exit.value } as const
    }
    if (!shouldRevertCause(exit.cause)) {
      // Pure defect path — propagate the cause unchanged.
      return yield* Effect.failCause(exit.cause) as unknown as Effect.Effect<
        CheckpointResult<A, E>,
        never,
        never
      >
    }
    yield* revertAgent(self, RevertTarget.toOplogIndex(checkpointIdx))
    return { _tag: "reverted", cause: exit.cause } as const
  })

/**
 * Saga-flavoured combinator: acquire a value, run a body with it, and
 * register a `compensate` Effect that runs only on body failure
 * (alongside a self-revert). The compensator is intended for external
 * side effects that the durable revert cannot undo (e.g. an HTTP call
 * to a remote system). `acquire` failures bubble up directly without
 * compensation; defects bubble up unchanged.
 *
 * @since 1.5.0
 * @category combinators
 */
export const compensable = <A, B, E, R>(input: {
  readonly acquire: Effect.Effect<A, E, R>
  readonly body: (a: A) => Effect.Effect<B, E, R>
  readonly compensate: (a: A) => Effect.Effect<void, unknown, R>
}): Effect.Effect<
  B,
  E | OplogHostError | AgentsHostError,
  R | SelfAgentId | OplogClient | AgentHostClient
> =>
  Effect.gen(function* () {
    const checkpointIdx = yield* currentIndex
    const self = yield* SelfAgentId
    const a = yield* input.acquire
    const exit = yield* Effect.exit(input.body(a))
    if (exit._tag === "Success") return exit.value
    if (!shouldRevertCause(exit.cause)) {
      // Defect — propagate unchanged, no compensation.
      return yield* Effect.failCause(exit.cause) as unknown as Effect.Effect<B, never, never>
    }
    // Run the user-supplied compensator; failures are swallowed so
    // the revert path always reaches the host.
    yield* input.compensate(a).pipe(Effect.ignore)
    return yield* revertAndSuspend(self, checkpointIdx)
  })
