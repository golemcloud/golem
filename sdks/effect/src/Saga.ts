/**
 * Effect-idiomatic saga / multi-step transaction module on top of the
 * Golem oplog primitives.
 *
 * The wire layout mirrors the official `golem-ts-sdk` /
 * `golem-rust-sdk` saga implementation:
 *
 * - on transaction entry, the current oplog index is captured as the
 *   "checkpoint" jump target;
 * - each step runs inside its own atomic region (`mark-begin-operation`
 *   / `mark-end-operation`);
 * - successful steps register a compensation effect on the surrounding
 *   {@link Scope.Scope} via `Scope.addFinalizer`, so the runtime
 *   automatically drains compensations in LIFO order on failure;
 * - on failure of an {@link infallibleTransaction}, after the
 *   compensations have run, the SDK calls `set-oplog-index(checkpoint)`
 *   and parks the fiber via `Effect.never` until the host preempts and
 *   replays from the checkpoint;
 * - on failure of a {@link fallibleTransaction}, the typed body error
 *   is wrapped into a {@link TransactionFailure} and returned. If a
 *   compensation registered through {@link withFallibleCompensation}
 *   itself fails, the result becomes
 *   `FailedAndRolledBackPartially`.
 *
 * Effect-canonical compensation registration via
 * {@link withCompensation} matches the `@effect/workflow` package's
 * `Workflow.withCompensation` shape exactly: an infallible
 * `Effect<void, never, R>` callback receiving `(value, cause)`. This
 * is the primary, idiomatic API and is composable across both
 * fallible and infallible transactions.
 *
 * For workflows where rollback itself is fallible — and the caller
 * needs to know that the system was left in an inconsistent state —
 * use {@link withFallibleCompensation} instead. Its compensation is
 * `Effect<void, E, R>`; the first such failure surfaces as
 * `FailedAndRolledBackPartially { error, compensationError }`.
 *
 * Defects (`Effect.die` / unexpected throws) are NOT routed through
 * the saga machinery — they propagate unchanged, matching the spirit
 * of `Durability.unwrapOrRevert` and `Durability.checkpoint`.
 *
 * Nested transactions on the same fiber tree are rejected with
 * {@link NestedSagaError}: the host's atomic-region bracketing and
 * the in-fiber checkpoint stack are inherently sequential, and
 * nesting would silently corrupt the comp-stack drain order.
 *
 * @since 0.1.0
 */

import { Cause, Context, Effect, Ref, Scope } from "effect"
import { atomically, DurabilityHostError } from "./internal/durabilityMode.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { OplogClient } from "./host/OplogClient.js"
import { currentIndex, OplogHostError, setIndex } from "./Oplog.js"

// ---------------------------------------------------------------------------
// Internal fiber-local references
// ---------------------------------------------------------------------------

/**
 * Per-transaction context: the active mode (so nesting / fallible-
 * compensation routing can branch) plus the captured-error sink for
 * fallible compensations.
 *
 * @internal
 */
interface SagaContextValue {
  readonly mode: "fallible" | "infallible"
  readonly pushCompensationError: (error: unknown) => Effect.Effect<void>
  readonly readCompensationError: Effect.Effect<unknown | undefined>
}

/**
 * Fiber-local marker for "this fiber tree is currently inside a
 * `Saga.fallibleTransaction` / `Saga.infallibleTransaction` call".
 * Mirrors the `InsideWrapRef` pattern in `src/durable-function.ts`.
 *
 * @internal
 */
const InsideSagaRef = Context.Reference<SagaContextValue | null>("effect-golem/saga/inside-saga", {
  defaultValue: () => null,
})

/**
 * Per-transaction store of the body's failure cause. Compensation
 * finalizers read this to decide whether to fire (matching the
 * `Exit.isFailure(exit)` gate in `Effect.acquireRelease`).
 *
 * @internal
 */
interface CauseStoreValue {
  cause: Cause.Cause<unknown> | null
}

/**
 * Fiber-local pointer to the active cause store (reset to `null`
 * outside any saga so accidental `withCompensation` outside a saga
 * is a structurally well-typed no-op).
 *
 * @internal
 */
const CauseStoreRef = Context.Reference<CauseStoreValue | null>("effect-golem/saga/cause-store", {
  defaultValue: () => null,
})

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when a `Saga.fallibleTransaction` or
 * `Saga.infallibleTransaction` is called inside a fiber that already
 * has a saga context active. Mirrors `NestedDurableFunctionError` in
 * spirit: the host's bracketing is sequential, so nesting cannot be
 * made safe without changing the wire model.
 *
 * @since 0.1.0
 * @category errors
 */
export class NestedSagaError {
  readonly _tag = "NestedSagaError"
  readonly message: string
  constructor(readonly outerMode: "fallible" | "infallible") {
    this.message = `NestedSagaError: cannot start a saga inside a ${outerMode} saga`
  }
}

/**
 * Tagged result of a {@link fallibleTransaction} that failed.
 *
 * - `FailedAndRolledBackCompletely` — every registered compensation
 *   ran to completion. The system is consistent again.
 * - `FailedAndRolledBackPartially` — at least one compensation
 *   registered through {@link withFallibleCompensation} itself failed.
 *   The first failure is exposed as `compensationError`. Subsequent
 *   compensations still run (best-effort), but only the first failure
 *   is surfaced.
 *
 * @since 0.1.0
 * @category models
 */
export type TransactionFailure<E> =
  | { readonly _tag: "FailedAndRolledBackCompletely"; readonly error: E }
  | {
      readonly _tag: "FailedAndRolledBackPartially"
      readonly error: E
      readonly compensationError: unknown
    }

// ---------------------------------------------------------------------------
// withCompensation — canonical, infallible-compensation combinator
// ---------------------------------------------------------------------------

/**
 * Register a compensation that runs only when the surrounding
 * {@link Saga.fallibleTransaction} or {@link Saga.infallibleTransaction}
 * has failed (typed failure or interruption). The compensation cannot
 * fail (`E = never`).
 *
 * Mirrors `@effect/workflow`'s `Workflow.withCompensation` shape:
 *
 * **Example**
 *
 * ```ts
 * yield* Saga.withCompensation(bookFlight, (booking, _cause) =>
 *   cancelFlight(booking.id).pipe(Effect.ignore),
 * )
 * ```
 *
 * Compensations run in REVERSE registration order, sequentially,
 * uninterruptibly (the surrounding scope's auto-close handles this).
 *
 * Outside any saga (no transaction in scope) the compensation is
 * silently dropped — the body still runs in an atomic region, so
 * the call is structurally safe.
 *
 * Available in both pipe-friendly and data-first overloads.
 *
 * @since 0.1.0
 * @category combinators
 */
export const withCompensation: {
  <A, R2>(
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, R2>,
  ): <E, R>(
    effect: Effect.Effect<A, E, R>,
  ) => Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient>
  <A, E, R, R2>(
    effect: Effect.Effect<A, E, R>,
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, R2>,
  ): Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient>
} = ((...args: Array<unknown>) => {
  if (args.length === 1) {
    const compensate = args[0] as (
      value: unknown,
      cause: Cause.Cause<unknown>,
    ) => Effect.Effect<void, never, unknown>
    return (effect: Effect.Effect<unknown, unknown, unknown>) =>
      withCompensationImpl(effect, compensate)
  }
  return withCompensationImpl(
    args[0] as Effect.Effect<unknown, unknown, unknown>,
    args[1] as (value: unknown, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, unknown>,
  )
}) as typeof withCompensation

const withCompensationImpl = <A, E, R, R2>(
  effect: Effect.Effect<A, E, R>,
  compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, R2>,
): Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient> =>
  Effect.uninterruptibleMask((restore) =>
    Effect.gen(function* () {
      const value = yield* restore(atomically(effect))
      const store = yield* Effect.service(CauseStoreRef)
      const context = yield* Effect.context<R2 | DurabilityModeClient>()
      const scope = yield* Effect.service(Scope.Scope)
      yield* Scope.addFinalizer(
        scope,
        Effect.suspend(() => {
          if (store === null || store.cause === null) return Effect.void
          return atomically(compensate(value, store.cause)).pipe(
            Effect.provide(context),
            Effect.ignore,
          )
        }),
      )
      return value
    }),
  )

// ---------------------------------------------------------------------------
// withFallibleCompensation — Golem extension: fallible compensation
// ---------------------------------------------------------------------------

/**
 * Like {@link withCompensation} but the compensation may itself fail
 * (`Effect<void, E2, R2>`). The first compensation failure observed in
 * a transaction is captured and surfaced as
 * {@link TransactionFailure.FailedAndRolledBackPartially.compensationError}.
 *
 * Subsequent compensations still execute on a best-effort basis — only
 * the FIRST failure is reported (matching the official Rust SDK's
 * behaviour and avoiding cascading-compensation noise).
 *
 * Outside any transaction the fallible-compensation channel is a
 * no-op (matching {@link withCompensation}'s outside-saga behaviour);
 * inside an {@link infallibleTransaction} the captured error is
 * dropped because the infallible path never returns a value the
 * caller could inspect.
 *
 * @since 0.1.0
 * @category combinators
 */
export const withFallibleCompensation: {
  <A, E2, R2>(
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
  ): <E, R>(
    effect: Effect.Effect<A, E, R>,
  ) => Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient>
  <A, E, R, E2, R2>(
    effect: Effect.Effect<A, E, R>,
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
  ): Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient>
} = ((...args: Array<unknown>) => {
  if (args.length === 1) {
    const compensate = args[0] as (
      value: unknown,
      cause: Cause.Cause<unknown>,
    ) => Effect.Effect<void, unknown, unknown>
    return (effect: Effect.Effect<unknown, unknown, unknown>) =>
      withFallibleCompensationImpl(effect, compensate)
  }
  return withFallibleCompensationImpl(
    args[0] as Effect.Effect<unknown, unknown, unknown>,
    args[1] as (
      value: unknown,
      cause: Cause.Cause<unknown>,
    ) => Effect.Effect<void, unknown, unknown>,
  )
}) as typeof withFallibleCompensation

const withFallibleCompensationImpl = <A, E, R, E2, R2>(
  effect: Effect.Effect<A, E, R>,
  compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
): Effect.Effect<A, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient> =>
  withCompensationImpl(effect, (value, cause) =>
    Effect.gen(function* () {
      const ctx = yield* Effect.service(InsideSagaRef)
      const exit = yield* Effect.exit(compensate(value, cause))
      if (exit._tag === "Failure" && ctx !== null) {
        const failures = exit.cause.reasons.filter(Cause.isFailReason)
        const error = failures.length > 0 ? failures[0]!.error : exit.cause
        yield* ctx.pushCompensationError(error)
      }
    }),
  )

// ---------------------------------------------------------------------------
// operation — reusable execute+compensate factory (Golem-SDK parity)
// ---------------------------------------------------------------------------

/**
 * Build a reusable saga step from a paired `(execute, compensate)`
 * record, mirroring the `Operation` type from the official `golem-ts`
 * / `golem-rust` SDKs.
 *
 * The returned function runs `execute(input)` inside an atomic region
 * and registers `compensate(input, output, cause)` via
 * {@link withCompensation}:
 *
 * **Example**
 *
 * ```ts
 * const bookFlight = Saga.operation({
 *   execute:    ({ flightId }: { flightId: string }) =>
 *     Effect.gen(function* () { ... }),
 *   compensate: ({ flightId }, booking, _cause) =>
 *     cancelFlight(booking.id).pipe(Effect.ignore),
 * })
 *
 * const booking = yield* bookFlight({ flightId: "AA1" })
 * ```
 *
 * The compensation is infallible (`Effect<void, never, R2>`); use
 * {@link withFallibleCompensation} directly if you need the
 * partial-rollback signal.
 *
 * @since 0.1.0
 * @category constructors
 */
export const operation = <In, Out, E, R, R2>(input: {
  readonly execute: (input: In) => Effect.Effect<Out, E, R>
  readonly compensate: (
    input: In,
    output: Out,
    cause: Cause.Cause<unknown>,
  ) => Effect.Effect<void, never, R2>
}): ((
  input: In,
) => Effect.Effect<Out, E | DurabilityHostError, R | R2 | Scope.Scope | DurabilityModeClient>) => {
  return (in_: In) =>
    withCompensation(input.execute(in_), (out, cause) => input.compensate(in_, out, cause))
}

// ---------------------------------------------------------------------------
// Internals shared by both transaction entry points
// ---------------------------------------------------------------------------

/**
 * Cause-classification policy for the saga entry points:
 *
 * - typed failure → trigger compensation drain
 * - interruption → trigger compensation drain (matches official SDKs'
 *   "RAII-on-panic" behaviour and Effect's expectation that
 *   compensations are interrupt-safe);
 * - defect → propagate unchanged, no compensation.
 *
 * @internal
 */
const shouldCompensateCause = <E>(cause: Cause.Cause<E>): boolean => {
  for (const reason of cause.reasons) {
    if (Cause.isFailReason(reason) || Cause.isInterruptReason(reason)) return true
  }
  return false
}

/**
 * Build the per-transaction context value, including the
 * compensation-error sink.
 *
 * @internal
 */
const makeSagaContextValue = (
  mode: "fallible" | "infallible",
): Effect.Effect<{
  readonly value: SagaContextValue
  readonly readError: Effect.Effect<unknown | undefined>
}> =>
  Ref.make<unknown | undefined>(undefined).pipe(
    Effect.map((ref) => ({
      value: {
        mode,
        pushCompensationError: (error: unknown) =>
          Ref.update(ref, (prev) => (prev === undefined ? error : prev)),
        readCompensationError: Ref.get(ref),
      },
      readError: Ref.get(ref),
    })),
  )

/**
 * Yield to the runtime once after `set-oplog-index` so the host has a
 * chance to preempt the fiber. The fiber then suspends until the host
 * tears it down and replays from the checkpoint.
 *
 * @internal
 */
const rewindAndSuspend = (checkpoint: bigint): Effect.Effect<never, OplogHostError, OplogClient> =>
  setIndex(checkpoint).pipe(Effect.andThen(Effect.never))

// ---------------------------------------------------------------------------
// fallibleTransaction
// ---------------------------------------------------------------------------

/**
 * Run `body` as a fallible saga.
 *
 * On success, returns the body's value. On typed failure, drains the
 * compensations registered via {@link withCompensation} /
 * {@link withFallibleCompensation} (LIFO, sequential, uninterruptible)
 * and returns a {@link TransactionFailure}:
 *
 * - `FailedAndRolledBackCompletely { error }` — every compensation
 *   completed without error.
 * - `FailedAndRolledBackPartially { error, compensationError }` — the
 *   first compensation registered with {@link withFallibleCompensation}
 *   failed; subsequent compensations still ran best-effort, but only
 *   the first failure is reported.
 *
 * Interruption propagates unchanged (the body's compensations still
 * run via the surrounding scope, but the outer result is interruption,
 * not a `TransactionFailure`).
 *
 * Defects (`Effect.die` / unexpected throws) propagate unchanged
 * without triggering compensation.
 *
 * Nesting another saga inside the body raises {@link NestedSagaError}.
 *
 * @since 0.1.0
 * @category constructors
 */
export const fallibleTransaction = <A, E, R>(
  body: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  TransactionFailure<E> | DurabilityHostError | OplogHostError | NestedSagaError,
  Exclude<R, Scope.Scope> | DurabilityModeClient | OplogClient
> =>
  Effect.gen(function* () {
    const existing = yield* Effect.service(InsideSagaRef)
    if (existing !== null) return yield* Effect.fail(new NestedSagaError(existing.mode))

    const { value: contextValue, readError } = yield* makeSagaContextValue("fallible")
    const causeStore: CauseStoreValue = { cause: null }

    return yield* Effect.scopedWith((scope) =>
      Effect.gen(function* () {
        const exit = yield* Effect.exit(
          (body as Effect.Effect<A, E, R | Scope.Scope>).pipe(
            Effect.provideService(InsideSagaRef, contextValue),
            Effect.provideService(CauseStoreRef, causeStore),
            Scope.provide(scope),
          ),
        )
        if (exit._tag === "Success") return exit.value

        if (!shouldCompensateCause(exit.cause)) {
          return yield* Effect.failCause(exit.cause)
        }

        // Make the cause visible to the registered compensations,
        // then close the scope to drain them in LIFO order.
        causeStore.cause = exit.cause as Cause.Cause<unknown>
        yield* Scope.close(scope, exit).pipe(Effect.orDie)

        const failures = exit.cause.reasons.filter(Cause.isFailReason) as Array<Cause.Fail<E>>
        if (failures.length === 0) {
          // Interrupt-only cause — propagate interruption unchanged.
          return yield* Effect.failCause(exit.cause)
        }
        const error = failures[0]!.error
        const compensationError = yield* readError
        if (compensationError !== undefined) {
          return yield* Effect.fail({
            _tag: "FailedAndRolledBackPartially",
            error,
            compensationError,
          } as const satisfies TransactionFailure<E>)
        }
        return yield* Effect.fail({
          _tag: "FailedAndRolledBackCompletely",
          error,
        } as const satisfies TransactionFailure<E>)
      }),
    )
  }) as Effect.Effect<
    A,
    TransactionFailure<E> | DurabilityHostError | OplogHostError | NestedSagaError,
    Exclude<R, Scope.Scope> | DurabilityModeClient | OplogClient
  >

// ---------------------------------------------------------------------------
// infallibleTransaction
// ---------------------------------------------------------------------------

/**
 * Run `body` as an infallible saga.
 *
 * The body's typed-error channel must be `never` — typed failures
 * inside operations must be folded into the rewind protocol, not
 * propagated as typed errors. Concretely:
 *
 * 1. Drain registered compensations in reverse order.
 * 2. Call `set-oplog-index(checkpoint)` to ask the host to replay from
 *    the captured checkpoint.
 * 3. Park the fiber via `Effect.never` until the host preempts.
 *
 * In other words, an infallible transaction "always succeeds" from
 * the caller's perspective: either the body returns a value, or the
 * worker is rewound and the whole transaction is retried.
 *
 * Caveats:
 *
 * - The `body` cannot use `Effect.fail(...)` directly (typed `E` is
 *   `never`). Express failures via operations whose `compensate`
 *   returns the rollback action; the saga machinery converts the
 *   step-level typed error into the rewind protocol.
 * - Persistence-level `persistNothing` weakens the guarantee — without
 *   the oplog, `set-oplog-index` cannot rewind anything.
 * - Defects (`Effect.die`) propagate as defects without triggering
 *   compensation, matching the official SDKs.
 *
 * Nesting another saga inside the body raises {@link NestedSagaError}.
 *
 * @since 0.1.0
 * @category constructors
 */
export const infallibleTransaction = <A, R>(
  body: Effect.Effect<A, never, R>,
): Effect.Effect<
  A,
  DurabilityHostError | OplogHostError | NestedSagaError,
  Exclude<R, Scope.Scope> | DurabilityModeClient | OplogClient
> =>
  Effect.gen(function* () {
    const existing = yield* Effect.service(InsideSagaRef)
    if (existing !== null) return yield* Effect.fail(new NestedSagaError(existing.mode))

    const checkpoint = yield* currentIndex
    const { value: contextValue } = yield* makeSagaContextValue("infallible")
    const causeStore: CauseStoreValue = { cause: null }

    return yield* Effect.scopedWith((scope) =>
      Effect.gen(function* () {
        const exit = yield* Effect.exit(
          (body as Effect.Effect<A, never, R | Scope.Scope>).pipe(
            Effect.provideService(InsideSagaRef, contextValue),
            Effect.provideService(CauseStoreRef, causeStore),
            Scope.provide(scope),
          ),
        )
        if (exit._tag === "Success") return exit.value

        if (!shouldCompensateCause(exit.cause)) {
          return yield* Effect.failCause(exit.cause)
        }

        causeStore.cause = exit.cause
        yield* Scope.close(scope, exit).pipe(Effect.orDie)

        return yield* rewindAndSuspend(checkpoint)
      }),
    )
  }) as Effect.Effect<
    A,
    DurabilityHostError | OplogHostError | NestedSagaError,
    Exclude<R, Scope.Scope> | DurabilityModeClient | OplogClient
  >
