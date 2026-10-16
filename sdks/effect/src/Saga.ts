/**
 * Effect-idiomatic sagas with guest-side compensation.
 * - successful steps register a compensation effect on the surrounding
 *   {@link Scope.Scope} via `Scope.addFinalizer`, so the runtime
 *   automatically drains compensations in LIFO order on failure;
 * - on failure of an {@link infallibleTransaction}, after the
 *   compensations have run, the SDK executes a new attempt. Each attempt
 *   appends new durable calls; no oplog entries are deleted;
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
 * **Failure-cause routing summary**
 *
 * | Cause class             | Fallible saga                              | Infallible saga                                       |
 * | ----------------------- | ------------------------------------------ | ----------------------------------------------------- |
 * | `Effect.fail` (typed E) | compensate (LIFO) → `TransactionFailure`   | compensate (LIFO) → new attempt |
 * | `Effect.interrupt`      | compensate (LIFO) → propagate interrupt    | compensate (LIFO) → propagate interrupt |
 * | `Effect.die` (defect)   | NO compensation → call `host.trap(reason)` | NO compensation, NO rewind → call `host.trap(reason)` |
 *
 * Defects intentionally bypass the typed protocol AND call the
 * host's uncatchable `trap` so user code outside the saga cannot
 * observe the unexpected throw with `Effect.catchAll` and continue
 * with a half-rolled-back state. This mirrors the
 * `catch (e) { trap(...); throw e }` pattern in `golem-ts-sdk`'s
 * `host/transaction.ts`. Interrupts are deliberately NOT trapped —
 * they are structured cancellation in Effect (`Effect.timeout`,
 * `race`, parent-scope teardown), and trapping would make those
 * patterns kill the worker.
 *
 * Nested transactions on the same fiber tree are rejected with
 * {@link NestedSagaError} to keep compensation ownership unambiguous.
 *
 * @since 1.5.0
 */

import { Cause, Context, Effect, Option, Ref, Scope } from "effect"
import { formatCauseForTrap } from "./internal/durabilityMode.js"
import { AgentHostClient } from "./host/AgentHostClient.js"

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
  readonly pushCompensationError: (cause: Cause.Cause<unknown>) => Effect.Effect<void>
}

/**
 * Fiber-local marker for "this fiber tree is currently inside a
 * `Saga.fallibleTransaction` / `Saga.infallibleTransaction` call".
 * A `Context.Reference` (Effect 4's replacement for `FiberRef`) is
 * the right primitive: child fibers inherit it from their parent,
 * but unrelated fibers see the default (`null`).
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
 * has a saga context active. Each compensation belongs to one saga.
 *
 * @since 1.5.0
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
 * @since 1.5.0
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
 * silently dropped; the body still runs normally.
 *
 * Available in both pipe-friendly and data-first overloads.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withCompensation: {
  <A, R2>(
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, R2>,
  ): <E, R>(effect: Effect.Effect<A, E, R>) => Effect.Effect<A, E, R | R2 | Scope.Scope>
  <A, E, R, R2>(
    effect: Effect.Effect<A, E, R>,
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, never, R2>,
  ): Effect.Effect<A, E, R | R2 | Scope.Scope>
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

const withCompensationImpl = <A, E, R, E2, R2>(
  effect: Effect.Effect<A, E, R>,
  compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
): Effect.Effect<A, E, R | R2 | Scope.Scope> =>
  Effect.uninterruptibleMask((restore) =>
    Effect.gen(function* () {
      const value = yield* restore(effect)
      const store = yield* Effect.service(CauseStoreRef)
      const context = yield* Effect.context<R2>()
      const saga = yield* Effect.service(InsideSagaRef)
      const scope = yield* Effect.service(Scope.Scope)
      yield* Scope.addFinalizer(
        scope,
        Effect.suspend(() => {
          if (store === null || store.cause === null) return Effect.void
          return Effect.exit(
            Effect.suspend(() => compensate(value, store.cause!)).pipe(Effect.provide(context)),
          ).pipe(
            Effect.flatMap((exit) =>
              exit._tag === "Failure" && saga !== null
                ? saga.pushCompensationError(exit.cause)
                : Effect.void,
            ),
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
 * inside an {@link infallibleTransaction} a failed compensation traps
 * instead of starting another attempt. Compensation defects trap in
 * both transaction modes.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withFallibleCompensation: {
  <A, E2, R2>(
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
  ): <E, R>(effect: Effect.Effect<A, E, R>) => Effect.Effect<A, E, R | R2 | Scope.Scope>
  <A, E, R, E2, R2>(
    effect: Effect.Effect<A, E, R>,
    compensate: (value: A, cause: Cause.Cause<unknown>) => Effect.Effect<void, E2, R2>,
  ): Effect.Effect<A, E, R | R2 | Scope.Scope>
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
): Effect.Effect<A, E, R | R2 | Scope.Scope> => withCompensationImpl(effect, compensate)

// ---------------------------------------------------------------------------
// operation — reusable execute+compensate factory (Golem-SDK parity)
// ---------------------------------------------------------------------------

/**
 * Build a reusable saga step from a paired `(execute, compensate)`
 * record, mirroring the `Operation` type from the official `golem-ts`
 * / `golem-rust` SDKs.
 *
 * The returned function runs `execute(input)` and registers
 * `compensate(input, output, cause)` via
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
 * @since 1.5.0
 * @category constructors
 */
export const operation = <In, Out, E, R, R2>(input: {
  readonly execute: (input: In) => Effect.Effect<Out, E, R>
  readonly compensate: (
    input: In,
    output: Out,
    cause: Cause.Cause<unknown>,
  ) => Effect.Effect<void, never, R2>
}): ((input: In) => Effect.Effect<Out, E, R | R2 | Scope.Scope>) => {
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
  return (
    !cause.reasons.some(Cause.isDieReason) &&
    cause.reasons.some((reason) => Cause.isFailReason(reason) || Cause.isInterruptReason(reason))
  )
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
  readonly readError: Effect.Effect<Option.Option<Cause.Cause<unknown>>>
}> =>
  Ref.make<Option.Option<Cause.Cause<unknown>>>(Option.none()).pipe(
    Effect.map((ref) => ({
      value: {
        mode,
        pushCompensationError: (cause: Cause.Cause<unknown>) =>
          Ref.update(ref, (prev) =>
            Option.some(Option.isSome(prev) ? Cause.combine(prev.value, cause) : cause),
          ),
      },
      readError: Ref.get(ref),
    })),
  )

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
 * @since 1.5.0
 * @category constructors
 */
export const fallibleTransaction = <A, E, R>(
  body: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  TransactionFailure<E> | NestedSagaError,
  Exclude<R, Scope.Scope> | AgentHostClient
> =>
  Effect.gen(function* () {
    const existing = yield* Effect.service(InsideSagaRef)
    if (existing !== null) return yield* Effect.fail(new NestedSagaError(existing.mode))

    const { value: contextValue, readError } = yield* makeSagaContextValue("fallible")
    const causeStore: CauseStoreValue = { cause: null }

    const scope = yield* Scope.make()
    return yield* Effect.uninterruptibleMask((restore) =>
      Effect.gen(function* () {
        const exit = yield* Effect.exit(
          restore(
            (body as Effect.Effect<A, E, R | Scope.Scope>).pipe(
              Effect.provideService(InsideSagaRef, contextValue),
              Effect.provideService(CauseStoreRef, causeStore),
              Scope.provide(scope),
            ),
          ),
        )
        if (exit._tag === "Success") return exit.value

        if (!shouldCompensateCause(exit.cause)) {
          // Defect path — call the host's uncatchable trap so user code
          // outside the saga cannot observe the unexpected throw and
          // continue with a half-rolled-back state. Mirrors the
          // `catch (e) { trap(...); throw e }` pattern in
          // `golem-ts-sdk` `host/transaction.ts`.
          const host = yield* AgentHostClient
          yield* Effect.sync(() =>
            host.trap(`fallibleTransaction failed: ${formatCauseForTrap(exit.cause)}`),
          )
          return yield* Effect.failCause(exit.cause)
        }

        // Make the cause visible to the registered compensations,
        // then close the scope to drain them in LIFO order.
        causeStore.cause = exit.cause as Cause.Cause<unknown>
        yield* Scope.close(scope, exit).pipe(Effect.orDie)

        const compensationFailure = yield* readError
        if (
          Option.isSome(compensationFailure) &&
          compensationFailure.value.reasons.some(Cause.isDieReason)
        ) {
          const host = yield* AgentHostClient
          yield* Effect.sync(() =>
            host.trap(
              `fallibleTransaction compensation failed: ${Cause.pretty(compensationFailure.value)}`,
            ),
          )
          return yield* Effect.failCause(compensationFailure.value as Cause.Cause<never>)
        }
        const failures = exit.cause.reasons.filter(Cause.isFailReason) as Array<Cause.Fail<E>>
        if (failures.length === 0) {
          // Interrupt-only cause — propagate interruption unchanged.
          return yield* Effect.failCause(exit.cause)
        }
        const error = failures[0]!.error
        if (Option.isSome(compensationFailure)) {
          const failure = compensationFailure.value.reasons.find(Cause.isFailReason)
          return yield* Effect.fail({
            _tag: "FailedAndRolledBackPartially",
            error,
            compensationError: failure ? failure.error : compensationFailure.value,
          } as const satisfies TransactionFailure<E>)
        }
        return yield* Effect.fail({
          _tag: "FailedAndRolledBackCompletely",
          error,
        } as const satisfies TransactionFailure<E>)
      }).pipe(Effect.onExit((exit) => Scope.close(scope, exit))),
    )
  }) as Effect.Effect<
    A,
    TransactionFailure<E> | NestedSagaError,
    Exclude<R, Scope.Scope> | AgentHostClient
  >

// ---------------------------------------------------------------------------
// infallibleTransaction
// ---------------------------------------------------------------------------

/**
 * Run `body` as an infallible saga.
 *
 * On typed failure, drain registered compensations in reverse order,
 * then execute a new attempt with a fresh compensation scope. Attempts
 * append durable calls and do not restore JavaScript state. Callers must
 * compensate state changes explicitly. Reusing an external idempotency
 * key after compensation can suppress the new operation.
 *
 * Cancellation drains compensations and propagates. Defects trap without
 * compensation. Neither failure mode rewinds or retries a guest trap.
 *
 * Nesting another saga inside the body raises {@link NestedSagaError}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const infallibleTransaction = <A, E, R>(
  body: Effect.Effect<A, E, R>,
): Effect.Effect<A, NestedSagaError, Exclude<R, Scope.Scope> | AgentHostClient> =>
  Effect.gen(function* () {
    const existing = yield* Effect.service(InsideSagaRef)
    if (existing !== null) return yield* Effect.fail(new NestedSagaError(existing.mode))

    while (true) {
      const { value: contextValue, readError } = yield* makeSagaContextValue("infallible")
      const causeStore: CauseStoreValue = { cause: null }

      const scope = yield* Scope.make()
      const attempt = yield* Effect.uninterruptibleMask((restore) =>
        Effect.gen(function* () {
          const exit = yield* Effect.exit(
            restore(
              (body as Effect.Effect<A, E, R | Scope.Scope>).pipe(
                Effect.provideService(InsideSagaRef, contextValue),
                Effect.provideService(CauseStoreRef, causeStore),
                Scope.provide(scope),
              ),
            ),
          )
          if (exit._tag === "Success") return { value: exit.value }

          if (!shouldCompensateCause(exit.cause)) {
            // Defect path — same trap protocol as `fallibleTransaction`.
            const host = yield* AgentHostClient
            yield* Effect.sync(() =>
              host.trap(`infallibleTransaction failed: ${formatCauseForTrap(exit.cause)}`),
            )
            return yield* Effect.failCause(exit.cause as Cause.Cause<never>)
          }

          causeStore.cause = exit.cause
          yield* Scope.close(scope, exit).pipe(Effect.orDie)

          const compensationFailure = yield* readError
          if (Option.isSome(compensationFailure)) {
            const host = yield* AgentHostClient
            yield* Effect.sync(() =>
              host.trap(
                `infallibleTransaction compensation failed: ${Cause.pretty(compensationFailure.value)}`,
              ),
            )
            return yield* Effect.failCause(compensationFailure.value as Cause.Cause<never>)
          }
          if (exit.cause.reasons.some(Cause.isInterruptReason)) {
            return yield* Effect.failCause(exit.cause as Cause.Cause<never>)
          }
          return undefined
        }).pipe(Effect.onExit((exit) => Scope.close(scope, exit))),
      )
      if (attempt !== undefined) return attempt.value
    }
  }) as Effect.Effect<A, NestedSagaError, Exclude<R, Scope.Scope> | AgentHostClient>
