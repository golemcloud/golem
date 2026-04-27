import { Cause, Effect, Scope } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"
import { revertAgent, RevertTarget, type AgentsHostError } from "./agents.js"
import { currentIndex, type OplogHostError } from "./oplog.js"
import { SelfAgentId } from "./self-agent-id.js"

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
 * Authoring model:
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
 */

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

export type { OplogIndex, Uuid } from "golem:api/host@1.5.0"

/**
 * Re-exported from `golem:api/host@1.5.0` as a type alias so the
 * value-level {@link PersistenceLevel} const namespace can keep the
 * un-suffixed name.
 */
export type PersistenceLevelValue = ApiHost.PersistenceLevel

type RawPersistenceLevel = ApiHost.PersistenceLevel
type RawOplogIndex = ApiHost.OplogIndex

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Raised when a `golem:api/host@1.5.0` execution-mode call throws unexpectedly. */
export class DurabilityHostError {
  readonly _tag = "DurabilityHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `DurabilityHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/** Raised when a user-supplied input to a durability call is out of range. */
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

/** Pure-data constructors for the WIT `persistence-level` variant. */
export const PersistenceLevel = {
  /** Persist nothing (treat the body as ephemeral, no replay/restore guarantees). */
  persistNothing: { tag: "persist-nothing" } as const,
  /** Persist remote side effects only (the default before "smart"). */
  persistRemoteSideEffects: { tag: "persist-remote-side-effects" } as const,
  /** Smart auto-detection (host default; recommended for most agents). */
  smart: { tag: "smart" } as const,
} as const

// ---------------------------------------------------------------------------
// Host-binding indirections
// ---------------------------------------------------------------------------

let getOplogPersistenceLevelImpl: () => RawPersistenceLevel = () =>
  ApiHost.getOplogPersistenceLevel()
let setOplogPersistenceLevelImpl: (next: RawPersistenceLevel) => void = (next) =>
  ApiHost.setOplogPersistenceLevel(next)
let getIdempotenceModeImpl: () => boolean = () => ApiHost.getIdempotenceMode()
let setIdempotenceModeImpl: (next: boolean) => void = (next) => ApiHost.setIdempotenceMode(next)
let oplogCommitImpl: (replicas: number) => void = (n) => ApiHost.oplogCommit(n)
let markBeginOperationImpl: () => RawOplogIndex = () => ApiHost.markBeginOperation()
let markEndOperationImpl: (begin: RawOplogIndex) => void = (b) => ApiHost.markEndOperation(b)
let generateIdempotencyKeyImpl: () => ApiHost.Uuid = () => ApiHost.generateIdempotencyKey()

/** @internal */
export const __setGetOplogPersistenceLevelForTest = (fn: () => RawPersistenceLevel): void => {
  getOplogPersistenceLevelImpl = fn
}
/** @internal */
export const __resetGetOplogPersistenceLevelForTest = (): void => {
  getOplogPersistenceLevelImpl = () => ApiHost.getOplogPersistenceLevel()
}
/** @internal */
export const __setSetOplogPersistenceLevelForTest = (
  fn: (next: RawPersistenceLevel) => void,
): void => {
  setOplogPersistenceLevelImpl = fn
}
/** @internal */
export const __resetSetOplogPersistenceLevelForTest = (): void => {
  setOplogPersistenceLevelImpl = (next) => ApiHost.setOplogPersistenceLevel(next)
}
/** @internal */
export const __setGetIdempotenceModeForTest = (fn: () => boolean): void => {
  getIdempotenceModeImpl = fn
}
/** @internal */
export const __resetGetIdempotenceModeForTest = (): void => {
  getIdempotenceModeImpl = () => ApiHost.getIdempotenceMode()
}
/** @internal */
export const __setSetIdempotenceModeForTest = (fn: (next: boolean) => void): void => {
  setIdempotenceModeImpl = fn
}
/** @internal */
export const __resetSetIdempotenceModeForTest = (): void => {
  setIdempotenceModeImpl = (next) => ApiHost.setIdempotenceMode(next)
}
/** @internal */
export const __setOplogCommitForTest = (fn: (replicas: number) => void): void => {
  oplogCommitImpl = fn
}
/** @internal */
export const __resetOplogCommitForTest = (): void => {
  oplogCommitImpl = (n) => ApiHost.oplogCommit(n)
}
/** @internal */
export const __setMarkBeginOperationForTest = (fn: () => RawOplogIndex): void => {
  markBeginOperationImpl = fn
}
/** @internal */
export const __resetMarkBeginOperationForTest = (): void => {
  markBeginOperationImpl = () => ApiHost.markBeginOperation()
}
/** @internal */
export const __setMarkEndOperationForTest = (fn: (begin: RawOplogIndex) => void): void => {
  markEndOperationImpl = fn
}
/** @internal */
export const __resetMarkEndOperationForTest = (): void => {
  markEndOperationImpl = (b) => ApiHost.markEndOperation(b)
}
/** @internal */
export const __setGenerateIdempotencyKeyForTest = (fn: () => ApiHost.Uuid): void => {
  generateIdempotencyKeyImpl = fn
}
/** @internal */
export const __resetGenerateIdempotencyKeyForTest = (): void => {
  generateIdempotencyKeyImpl = () => ApiHost.generateIdempotencyKey()
}

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/** Read the current persistence level. */
export const getPersistenceLevel: Effect.Effect<RawPersistenceLevel, DurabilityHostError> =
  Effect.try({
    try: () => getOplogPersistenceLevelImpl(),
    catch: (e) => new DurabilityHostError(e),
  })

/** Write the persistence level. Persists to the oplog. */
export const setPersistenceLevel = (
  level: RawPersistenceLevel,
): Effect.Effect<void, DurabilityHostError> =>
  Effect.try({
    try: () => setOplogPersistenceLevelImpl(level),
    catch: (e) => new DurabilityHostError(e),
  })

/** Read the current idempotence mode. `true` = at-least-once; `false` = at-most-once. */
export const getIdempotenceMode: Effect.Effect<boolean, DurabilityHostError> = Effect.try({
  try: () => getIdempotenceModeImpl(),
  catch: (e) => new DurabilityHostError(e),
})

/** Write the idempotence mode. */
export const setIdempotenceMode = (idempotent: boolean): Effect.Effect<void, DurabilityHostError> =>
  Effect.try({
    try: () => setIdempotenceModeImpl(idempotent),
    catch: (e) => new DurabilityHostError(e),
  })

/**
 * Block until the oplog has been written to at least `replicas`
 * replicas (or the maximum if `replicas` exceeds the maximum). The host
 * type is `u8`, so `replicas` is validated to fit `0..=255`.
 */
export const oplogCommit = (
  replicas: number,
): Effect.Effect<void, DurabilityHostError | DurabilityValidationError> => {
  if (!Number.isSafeInteger(replicas) || replicas < 0 || replicas > 0xff) {
    return Effect.fail(
      new DurabilityValidationError(
        `oplogCommit.replicas must be an integer in 0..=255 (got ${String(replicas)})`,
      ),
    )
  }
  return Effect.try({
    try: () => oplogCommitImpl(replicas),
    catch: (e) => new DurabilityHostError(e),
  })
}

/**
 * Mark the beginning of an atomic region. Returns the host's chosen
 * `OplogIndex` to be passed back into {@link endOperation}. Prefer
 * {@link atomically} or {@link markAtomicOperationScoped} unless you
 * really need imperative control.
 */
export const beginOperation: Effect.Effect<RawOplogIndex, DurabilityHostError> = Effect.try({
  try: () => markBeginOperationImpl(),
  catch: (e) => new DurabilityHostError(e),
})

/**
 * Mark the end of the atomic region whose begin index is `begin`.
 * Idempotent on the host side: subsequent calls with the same index
 * are no-ops.
 */
export const endOperation = (begin: RawOplogIndex): Effect.Effect<void, DurabilityHostError> =>
  Effect.try({
    try: () => markEndOperationImpl(begin),
    catch: (e) => new DurabilityHostError(e),
  })

/**
 * Generate a fresh idempotency key. This call is committed to the
 * oplog so the same key is returned on replay — making the value safe
 * to use against external systems' idempotence checks (e.g. payment
 * gateways).
 */
export const generateIdempotencyKey: Effect.Effect<ApiHost.Uuid, DurabilityHostError> = Effect.try({
  try: () => generateIdempotencyKeyImpl(),
  catch: (e) => new DurabilityHostError(e),
})

// ---------------------------------------------------------------------------
// Scoped activation
// ---------------------------------------------------------------------------

/**
 * Acquire-release pair that sets the persistence level on entry and
 * restores the previous value on scope close (success, failure or
 * interruption).
 */
export const usePersistenceLevelScoped = (
  level: RawPersistenceLevel,
): Effect.Effect<void, DurabilityHostError, Scope.Scope> =>
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
 */
export const withPersistenceLevel = <A, E, R>(
  level: RawPersistenceLevel,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | DurabilityHostError, Exclude<R, Scope.Scope>> =>
  Effect.scoped(usePersistenceLevelScoped(level).pipe(Effect.andThen(effect)))

/**
 * Acquire-release pair that sets the idempotence mode on entry and
 * restores the previous value on scope close.
 */
export const useIdempotenceModeScoped = (
  idempotent: boolean,
): Effect.Effect<void, DurabilityHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const previous = yield* getIdempotenceMode
    yield* Effect.acquireRelease(setIdempotenceMode(idempotent), () =>
      setIdempotenceMode(previous).pipe(Effect.ignore),
    )
  })

/** Run `effect` with `idempotent` temporarily installed as the idempotence mode. */
export const withIdempotenceMode = <A, E, R>(
  idempotent: boolean,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | DurabilityHostError, Exclude<R, Scope.Scope>> =>
  Effect.scoped(useIdempotenceModeScoped(idempotent).pipe(Effect.andThen(effect)))

/**
 * Open an atomic region scoped to the surrounding {@link Scope}. On
 * acquire calls `mark-begin-operation`; on release (success, failure,
 * or interruption) calls `mark-end-operation` with the captured begin
 * index. Returns the begin index for callers that need it.
 */
export const markAtomicOperationScoped: Effect.Effect<
  RawOplogIndex,
  DurabilityHostError,
  Scope.Scope
> = Effect.acquireRelease(beginOperation, (begin) => endOperation(begin).pipe(Effect.ignore))

/**
 * Run `effect` inside an atomic region. Equivalent to
 * `Effect.scoped(markAtomicOperationScoped.pipe(Effect.andThen(effect)))`.
 * If `effect` fails or is interrupted, the host treats the region as
 * needing reexecution on the next replay.
 */
export const atomically = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | DurabilityHostError, Exclude<R, Scope.Scope>> =>
  Effect.scoped(markAtomicOperationScoped.pipe(Effect.andThen(effect)))

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
): Effect.Effect<never, AgentsHostError, never> =>
  revertAgent(self, RevertTarget.toOplogIndex(checkpointIdx)).pipe(Effect.andThen(Effect.never))

/**
 * Run a fallible Effect; if it fails (typed failure) or is
 * interrupted, revert this agent's oplog to the index captured before
 * the body ran. The host preempts and restarts the agent from the
 * checkpoint, so any durable side effects performed inside `effect`
 * "never happened" from the post-revert perspective.
 *
 * Defects (`Effect.die` / unexpected throws) are **not** routed to
 * revert — they propagate as defects, matching the spirit of the
 * official SDKs' `unwrap-or-revert`.
 *
 * Mirrors `golem-ts-sdk` / `golem-rust-sdk`. The returned Effect's
 * typed failures are: oplog/agent host bookkeeping errors only — the
 * body's `E` channel is suppressed because the failure branch never
 * resumes (revert is followed by `Effect.never`).
 */
export const unwrapOrRevert = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, OplogHostError | AgentsHostError, R | SelfAgentId> =>
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
 * In real Golem runtimes the `revertAgent` call typically preempts
 * before the tagged result is observed; this combinator is most
 * useful inside test runtimes that do not preempt on revert and for
 * callers that want to inspect the post-revert state.
 */
export const checkpoint = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<CheckpointResult<A, E>, OplogHostError | AgentsHostError, R | SelfAgentId> =>
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
 */
export const compensable = <A, B, E, R>(input: {
  readonly acquire: Effect.Effect<A, E, R>
  readonly body: (a: A) => Effect.Effect<B, E, R>
  readonly compensate: (a: A) => Effect.Effect<void, unknown, R>
}): Effect.Effect<B, E | OplogHostError | AgentsHostError, R | SelfAgentId> =>
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
