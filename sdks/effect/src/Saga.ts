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
 * All catchable causes drain compensation under an interruption mask.
 * Fallible sagas preserve defects and interruption after complete rollback.
 * Infallible sagas ask the host for a bounded retry decision after complete
 * rollback; exhaustion and policy errors are typed `SagaRetryFailure` values.
 *
 * Nested transactions on the same fiber tree are rejected with
 * {@link NestedSagaError} to keep compensation ownership unambiguous.
 *
 * @since 1.5.0
 */

import { Cause, Context, Effect, Option, Ref, Scope } from "effect"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RetryClient } from "./host/RetryClient.js"
import type { PredicateValue } from "golem:api/retry@1.5.0"
import { uuidToString } from "golem:core/types@2.0.0"

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

/**
 * An exhausted or cancelled retry, invalid policy, or incomplete compensation.
 * @since 1.5.0
 * @category errors
 */
export class SagaRetryFailure {
  readonly _tag = "SagaRetryFailure"
  constructor(
    readonly cause: Cause.Cause<unknown>,
    readonly compensationError?: Cause.Cause<unknown>,
    readonly policyError?: unknown,
  ) {}
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

// Restore boundaries in Effect 4 replace the exit with a pending external
// interruption. Keep the saga outcome outside those boundaries. The exit
// finalizer polls pending cancellation and combines it with the saved outcome.
const preserveInterruptedOutcome = <A, E, R>(
  run: (remember: (cause: Cause.Cause<unknown> | undefined) => void) => Effect.Effect<A, E, R>,
): Effect.Effect<A, E, R> =>
  Effect.suspend(() => {
    let outcome: Cause.Cause<unknown> | undefined
    let pollCancellation: Effect.Effect<void> = Effect.void
    return Effect.uninterruptibleMask((restore) => {
      pollCancellation = restore(Effect.void)
      return restore(
        run((cause) => {
          outcome = cause
        }),
      )
    }).pipe(
      Effect.onExit(() =>
        Effect.gen(function* () {
          const cancellation = yield* Effect.exit(pollCancellation)
          if (cancellation._tag === "Failure" && outcome !== undefined) {
            return yield* Effect.failCause(Cause.combine(cancellation.cause, outcome))
          }
        }),
      ),
    ) as Effect.Effect<A, E, R>
  })

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
 * External cancellation retains interruption together with the rollback
 * outcome in the final cause. Partial rollback retains both the body cause
 * and the compensation failure.
 *
 * Defects (`Effect.die` / unexpected throws) propagate unchanged after
 * compensation. Partial rollback retains the original cause in `error`.
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
  TransactionFailure<E | Cause.Cause<E>> | NestedSagaError,
  Exclude<R, Scope.Scope> | AgentHostClient
> =>
  preserveInterruptedOutcome((remember) =>
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

          // Make the cause visible to the registered compensations,
          // then close the scope to drain them in LIFO order.
          causeStore.cause = exit.cause as Cause.Cause<unknown>
          yield* Scope.close(scope, exit).pipe(Effect.orDie)

          const compensationFailure = yield* readError
          const failures = exit.cause.reasons.filter(Cause.isFailReason) as Array<Cause.Fail<E>>
          const unexpected = exit.cause.reasons.some(
            (reason) => Cause.isDieReason(reason) || Cause.isInterruptReason(reason),
          )
          if (unexpected && Option.isNone(compensationFailure)) {
            return yield* Effect.failCause(exit.cause)
          }
          const error = !unexpected && failures.length > 0 ? failures[0]!.error : exit.cause
          if (Option.isSome(compensationFailure)) {
            const failure = compensationFailure.value.reasons.find(Cause.isFailReason)
            return yield* Effect.fail({
              _tag: "FailedAndRolledBackPartially",
              error,
              compensationError:
                failure && compensationFailure.value.reasons.every(Cause.isFailReason)
                  ? failure.error
                  : compensationFailure.value,
            } as const satisfies TransactionFailure<E | Cause.Cause<E>>)
          }
          return yield* Effect.fail({
            _tag: "FailedAndRolledBackCompletely",
            error,
          } as const satisfies TransactionFailure<E | Cause.Cause<E>>)
        }).pipe(
          Effect.onExit((exit) => Scope.close(scope, exit)),
          Effect.onExit((exit) =>
            Effect.sync(() => {
              if (exit._tag === "Failure") remember(exit.cause)
            }),
          ),
        ),
      )
    }),
  ) as Effect.Effect<
    A,
    TransactionFailure<E | Cause.Cause<E>> | NestedSagaError,
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
 * Caught defects and self-interruption also drain compensation before the host
 * retry decision. External cancellation stops retries and retains the latest
 * body cause and rollback outcome together with interruption in the final cause.
 * Uncatchable guest traps cannot run guest compensation.
 * The host owns policy evaluation, jitter, continuation state and bounds.
 *
 * Nesting another saga inside the body raises {@link NestedSagaError}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const infallibleTransaction = <A, E, R>(
  body: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  NestedSagaError | SagaRetryFailure,
  Exclude<R, Scope.Scope> | AgentHostClient | RetryClient
> =>
  preserveInterruptedOutcome((remember) =>
    Effect.gen(function* () {
      const existing = yield* Effect.service(InsideSagaRef)
      if (existing !== null) return yield* Effect.fail(new NestedSagaError(existing.mode))

      const retry = yield* RetryClient
      const host = yield* AgentHostClient
      const started = yield* Effect.sync(() => retry.now())
      let state: Uint8Array = new Uint8Array()
      let failedAttempts = 0n
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
            if (exit._tag === "Success") {
              remember(undefined)
              return { value: exit.value }
            }

            causeStore.cause = exit.cause
            remember(Cause.fail(new SagaRetryFailure(exit.cause)))
            yield* Scope.close(scope, exit).pipe(Effect.orDie)

            const compensationFailure = yield* readError
            if (Option.isSome(compensationFailure)) {
              return yield* Effect.fail(new SagaRetryFailure(exit.cause, compensationFailure.value))
            }
            const decision = yield* Effect.try({
              try: () => {
                const id = host.getSelfMetadata().agentId
                const noun = `urn:worker:${uuidToString(id.componentId.uuid)}/${id.agentId}`
                const kind = exit.cause.reasons.some(Cause.isInterruptReason)
                  ? "interrupted"
                  : exit.cause.reasons.some(Cause.isDieReason)
                    ? "exception"
                    : "operation-error"
                const properties: [string, PredicateValue][] = [
                  ["attempt", { tag: "integer", val: ++failedAttempts }],
                  ["error-kind", { tag: "text", val: kind }],
                ]
                const policy = retry.resolveRetryPolicy("saga", noun, properties)
                const elapsed = retry.now() - started
                return retry.nextRetryDelay(
                  policy,
                  state,
                  elapsed < 0n ? 0n : elapsed,
                  "saga",
                  noun,
                  properties,
                )
              },
              catch: (error) => new SagaRetryFailure(exit.cause, undefined, error),
            })
            state = decision.state
            if (decision.delay === undefined)
              return yield* Effect.fail(new SagaRetryFailure(exit.cause))
            return { delay: decision.delay }
          }).pipe(
            Effect.onExit((exit) => Scope.close(scope, exit)),
            Effect.onExit((exit) =>
              Effect.sync(() => {
                if (exit._tag === "Failure") remember(exit.cause)
              }),
            ),
          ),
        )
        if ("value" in attempt) return attempt.value
        let milliseconds = (attempt.delay + 999_999n) / 1_000_000n
        while (milliseconds > 0n) {
          const chunk = Number(milliseconds > 2_147_483_647n ? 2_147_483_647n : milliseconds)
          yield* Effect.sleep(chunk)
          milliseconds -= BigInt(chunk)
        }
      }
    }),
  ) as Effect.Effect<
    A,
    NestedSagaError | SagaRetryFailure,
    Exclude<R, Scope.Scope> | AgentHostClient | RetryClient
  >
