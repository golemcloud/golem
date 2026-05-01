/**
 * Effect-idiomatic façade over `golem:quota/types@1.5.0`.
 *
 * Mirrors the `quota` surface shipped by the official `golem-ts-sdk` /
 * `golem-rust-sdk` (`acquireQuotaToken`, `withReservation`, `split`,
 * `merge`, manual `reserve` + `commit`) but exposes every host
 * interaction as an Effect with typed failures and uses Effect's
 * `Scope` to make reservation lifetimes leak-safe by construction.
 *
 * **Example**
 *
 * ```ts
 * import { Quota } from "effect-golem"
 * import { Effect } from "effect"
 *
 * const useApi = Effect.gen(function* () {
 *   const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
 *
 *   // Manual reserve / commit (scope-safe — drop ≡ commit(0)):
 *   yield* Effect.scoped(
 *     Effect.gen(function* () {
 *       const reservation = yield* Quota.reserve(token, 100n)
 *       const result = yield* doWork()
 *       yield* Quota.commit(reservation, BigInt(result.actualUsage))
 *     }),
 *   )
 *
 *   // Or, idiomatic RAII-style:
 *   const value = yield* Quota.withReservation(token, 4000n, (_r) =>
 *     Effect.gen(function* () {
 *       const response = yield* callLlm(prompt, { maxTokens: 4000 })
 *       return { used: BigInt(response.tokensUsed), value: response }
 *     }),
 *   )
 * })
 * ```
 *
 * The Schema codec for `QuotaToken` (and `QuotaTokenRecord`) keeps
 * working unchanged — passing a token across an RPC boundary continues
 * to use `toRecord` / `fromRecord` under the hood.
 *
 * @since 0.1.0
 */
import { Cause, Effect, Exit, Schema, SchemaGetter, Scope } from "effect"
import * as QuotaHost from "golem:quota/types@1.5.0"
import { QuotaClient } from "./host/QuotaClient.js"
import { Int64, Uint32, Uint64 } from "./WitTypes.js"

// ---------------------------------------------------------------------------
// Re-exported runtime handle types
// ---------------------------------------------------------------------------

/**
 * Opaque handle to a host quota token. Acquired via {@link acquireQuotaToken},
 * carried by value across RPC (via the {@link QuotaToken} schema codec
 * defined below), and consumed by {@link reserve} / {@link withReservation} /
 * {@link split} / {@link merge}.
 *
 * NOTE: this type alias is *type-only* — it deliberately co-exists with
 * the value `QuotaToken` (the schema codec) under the same name, in
 * different namespaces. TypeScript merges the two: `QuotaToken` in a
 * type position refers to the host class instance; `QuotaToken` in a
 * value position refers to the schema codec.
 *
 * @since 0.1.0
 * @category models
 */
export type QuotaToken = QuotaHost.QuotaToken

/**
 * Opaque, scope-managed reservation handle. Returned by {@link reserve}
 * and given to the body of {@link withReservation}; commit usage with
 * {@link commit}, or let the surrounding `Scope` close (drop ≡
 * `commit(0)` per the WIT contract).
 *
 * @since 0.1.0
 * @category models
 */
export interface Reservation {
  readonly [ReservationTypeId]: typeof ReservationTypeId
}

/**
 * Brand symbol identifying SDK-owned reservations.
 *
 * @since 0.1.0
 * @category symbols
 */
export const ReservationTypeId: unique symbol = Symbol.for("@effect-golem/quota/Reservation")
/**
 * @since 0.1.0
 * @category symbols
 */
export type ReservationTypeId = typeof ReservationTypeId

interface ReservationImpl extends Reservation {
  readonly raw: QuotaHost.Reservation
  /** Set to `true` after a successful explicit commit; the scope finalizer
   *  treats already-committed reservations as no-ops. */
  committed: boolean
}

const makeReservation = (raw: QuotaHost.Reservation): ReservationImpl => ({
  [ReservationTypeId]: ReservationTypeId,
  raw,
  committed: false,
})

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when a {@link reserve} call cannot be satisfied because the
 * resource's enforcement policy is `reject`. Mirrors WIT
 * `failed-reservation`.
 *
 * **Details**
 *
 * `estimatedWaitNanos` is set only when the host can estimate how long
 * the caller would need to wait for capacity (rate-limited resources);
 * `undefined` for `reject` enforcement on quota-based resources.
 *
 * @since 0.1.0
 * @category errors
 */
export class FailedReservationError {
  readonly _tag = "FailedReservationError"
  readonly message: string
  readonly estimatedWaitNanos: bigint | undefined
  constructor(raw: QuotaHost.FailedReservation) {
    this.estimatedWaitNanos = raw.estimatedWaitNanos
    this.message =
      raw.estimatedWaitNanos === undefined
        ? "FailedReservationError: reservation rejected by enforcement policy"
        : `FailedReservationError: reservation rejected (estimated wait: ${raw.estimatedWaitNanos}ns)`
  }
  /** JSON-safe rendering — `estimatedWaitNanos` is folded into the
   *  message so a `Cause` containing this error can be serialized
   *  without tripping over the `bigint`. */
  toJSON() {
    return {
      _tag: this._tag,
      message: this.message,
      estimatedWaitNanos:
        this.estimatedWaitNanos === undefined ? undefined : this.estimatedWaitNanos.toString(),
    }
  }
}

/**
 * Raised when a `golem:quota/types@1.5.0` host call throws unexpectedly
 * (i.e. anything that is *not* a `failed-reservation`). Examples:
 * dropping a reservation that the host has already invalidated,
 * committing twice, calling `split` / `merge` with arguments that the
 * host rejects with a panic.
 *
 * @since 0.1.0
 * @category errors
 */
export class QuotaHostError {
  readonly _tag = "QuotaHostError"
  readonly message: string
  constructor(
    readonly operation: string,
    readonly cause: unknown,
  ) {
    this.message = `QuotaHostError(${operation}): ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Schema for the wire (RPC) shape — preserved for backward compatibility.
// ---------------------------------------------------------------------------

/**
 * Schema for `golem:core/types@1.5.0`.Uuid — a 128-bit value carried as
 * two 64-bit halves.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Uuid = Schema.Struct({
  highBits: Uint64,
  lowBits: Uint64,
})

/**
 * Schema for `golem:api/host@1.5.0`.EnvironmentId.
 *
 * @since 0.1.0
 * @category codecs
 */
export const EnvironmentId = Schema.Struct({
  uuid: Uuid,
})

/**
 * Schema for `wasi:clocks/wall-clock@0.2.3`.Datetime.
 *
 * @since 0.1.0
 * @category codecs
 */
export const Datetime = Schema.Struct({
  seconds: Int64,
  nanoseconds: Uint32,
})

/**
 * Schema for the wire shape of a `QuotaToken` — the record returned by
 * `QuotaToken.toRecord()` and accepted by `QuotaToken.fromRecord()`.
 *
 * @since 0.1.0
 * @category codecs
 */
export const QuotaTokenRecord = Schema.Struct({
  environmentId: EnvironmentId,
  resourceName: Schema.String,
  expectedUse: Uint64,
  lastCredit: Int64,
  lastCreditAt: Datetime,
})

/**
 * Schema for the host `QuotaToken` class. The decoded `Type` is the
 * runtime `QuotaToken` instance; the `Encoded` form is `QuotaTokenRecord`,
 * which the codec then maps onto the corresponding WIT record.
 *
 * **Details**
 *
 * Bridging is done by `QuotaToken.fromRecord` / `QuotaToken.toRecord` —
 * the official host class invariants are preserved end-to-end.
 *
 * @since 0.1.0
 * @category codecs
 */
export const QuotaToken: Schema.Codec<
  QuotaHost.QuotaToken,
  typeof QuotaTokenRecord.Encoded,
  never,
  never
> = QuotaTokenRecord.pipe(
  Schema.decodeTo(
    Schema.declare((u): u is QuotaHost.QuotaToken => u instanceof QuotaHost.QuotaToken),
    {
      decode: SchemaGetter.transform((rec: typeof QuotaTokenRecord.Type) =>
        QuotaHost.QuotaToken.fromRecord(rec),
      ),
      encode: SchemaGetter.transform((tok: QuotaHost.QuotaToken) => tok.toRecord()),
    },
  ),
)

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/**
 * Heuristic for distinguishing a thrown `failed-reservation` (typed
 * domain failure) from any other host trap. The WIT shape is `record
 * failed-reservation { estimated-wait-nanos: option<u64> }`, so the
 * presence of the field — even when its value is `undefined` — uniquely
 * identifies the case.
 */
const isFailedReservation = (e: unknown): e is QuotaHost.FailedReservation =>
  typeof e === "object" && e !== null && "estimatedWaitNanos" in e

/**
 * Acquire a quota token for the given resource. Mirrors the reference
 * SDKs' `acquireQuotaToken(name, expectedUse)`.
 *
 * **Details**
 *
 * - `resourceName` must match a key declared in the agent's manifest
 *   `resourceDefaults`.
 * - `expectedUse` is the typical units per reservation; the host uses
 *   it to derive credit rate / max-credit for fair scheduling.
 *
 * @since 0.1.0
 * @category constructors
 */
export const acquireQuotaToken = (
  resourceName: string,
  expectedUse: bigint,
): Effect.Effect<QuotaToken, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const client = yield* QuotaClient
    return yield* Effect.try({
      try: () => client.acquireQuotaToken(resourceName, expectedUse),
      catch: (cause) => new QuotaHostError("acquireQuotaToken", cause),
    })
  })

/**
 * Reserve `amount` units from the local allocation. The returned
 * {@link Reservation} is bound to the surrounding {@link Scope} —
 * closing the scope without an explicit {@link commit} is equivalent
 * to `commit(0)` (per the WIT contract).
 *
 * **Details**
 *
 * Fails with {@link FailedReservationError} when the resource's
 * enforcement policy is `reject` (`throttle` / `terminate` policies are
 * handled inside the host before `reserve` returns).
 *
 * @since 0.1.0
 * @category operations
 */
export const reserve = (
  token: QuotaToken,
  amount: bigint,
): Effect.Effect<Reservation, FailedReservationError | QuotaHostError, Scope.Scope | QuotaClient> =>
  Effect.gen(function* () {
    const client = yield* QuotaClient
    return yield* Effect.acquireRelease(
      Effect.try({
        try: () => makeReservation(client.reserve(token, amount)),
        catch: (cause) => {
          if (isFailedReservation(cause)) {
            return new FailedReservationError(cause)
          }
          return new QuotaHostError("reserve", cause)
        },
      }),
      (reservation) =>
        Effect.sync(() => {
          const r = reservation as ReservationImpl
          if (!r.committed) {
            try {
              client.commit(r.raw, 0n)
            } catch {
              // Scope finalizers must not throw; absorb host errors
              // during best-effort cleanup. An explicit `commit` would
              // have surfaced this as `QuotaHostError`.
            }
            r.committed = true
          }
        }),
    )
  })

/**
 * Commit actual usage. Mirrors WIT `reservation.commit(used)`:
 *
 * **Details**
 *
 * - `used < reserved` → unused capacity is returned to the pool.
 * - `used > reserved` → the excess is deducted from the token's
 *    remaining allocation as "debt".
 *
 * Calling `commit` twice on the same reservation fails with
 * {@link QuotaHostError}. Calling it once consumes the reservation; the
 * surrounding scope's finalizer becomes a no-op.
 *
 * @since 0.1.0
 * @category operations
 */
export const commit = (
  reservation: Reservation,
  used: bigint,
): Effect.Effect<void, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const r = reservation as ReservationImpl
    if (r.committed) {
      return yield* Effect.fail(
        new QuotaHostError("commit", new Error("Reservation already committed")),
      )
    }
    const client = yield* QuotaClient
    return yield* Effect.try({
      try: () => {
        client.commit(r.raw, used)
        r.committed = true
      },
      catch: (cause) => new QuotaHostError("commit", cause),
    })
  })

/**
 * Split a child token off `token` with `childExpectedUse` units. The
 * parent's `expectedUse` is reduced by `childExpectedUse`; credits are
 * divided proportionally. The host TRAPS if `childExpectedUse` exceeds
 * the parent's current `expectedUse` — that surfaces as a
 * {@link QuotaHostError}.
 *
 * @since 0.1.0
 * @category operations
 */
export const split = (
  token: QuotaToken,
  childExpectedUse: bigint,
): Effect.Effect<QuotaToken, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const client = yield* QuotaClient
    return yield* Effect.try({
      try: () => client.split(token, childExpectedUse),
      catch: (cause) => new QuotaHostError("split", cause),
    })
  })

/**
 * Merge `other` back into `token`. The host TRAPS if the two tokens
 * refer to different resources — that surfaces as a
 * {@link QuotaHostError}. After a successful merge, `other` is
 * consumed and must not be used again.
 *
 * @since 0.1.0
 * @category operations
 */
export const merge = (
  token: QuotaToken,
  other: QuotaToken,
): Effect.Effect<void, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const client = yield* QuotaClient
    return yield* Effect.try({
      try: () => client.merge(token, other),
      catch: (cause) => new QuotaHostError("merge", cause),
    })
  })

/**
 * RAII-style helper: reserves `amount` units, hands the live
 * {@link Reservation} to `body`, and on success commits the body's
 * declared `used` count. On body failure / interruption / defect, the
 * reservation falls back to `commit(0)` via the surrounding scope
 * finalizer.
 *
 * **Details**
 *
 * The body must return `{ used, value }`. The `used` field is the
 * actual consumption to commit (typically derived from the body's
 * result); `value` is the value `withReservation` resolves to.
 *
 * **Example**
 *
 * ```ts
 * const result = yield* Quota.withReservation(token, 4000n, (_r) =>
 *   Effect.gen(function* () {
 *     const response = yield* callLlm(prompt, { maxTokens: 4000 })
 *     return { used: BigInt(response.tokensUsed), value: response }
 *   }),
 * )
 * ```
 *
 * @since 0.1.0
 * @category combinators
 */
export const withReservation = <A, E, R>(
  token: QuotaToken,
  amount: bigint,
  body: (reservation: Reservation) => Effect.Effect<{ used: bigint; value: A }, E, R>,
): Effect.Effect<A, E | FailedReservationError | QuotaHostError, R | QuotaClient> =>
  Effect.scoped(
    Effect.gen(function* () {
      const reservation = yield* reserve(token, amount)
      const exit = yield* Effect.exit(body(reservation))
      if (Exit.isSuccess(exit)) {
        const { used, value } = exit.value
        yield* commit(reservation, used)
        return value
      }
      // On failure / defect / interruption the scope finalizer will
      // commit 0; just propagate the body's failure cause unchanged.
      return yield* Effect.failCause(exit.cause as Cause.Cause<E | QuotaHostError>)
    }),
  )

// ---------------------------------------------------------------------------
// Re-exports of raw WIT types (no re-export of the host classes; use
// the Effect-typed wrappers above instead).
// ---------------------------------------------------------------------------

/**
 * @since 0.1.0
 * @category re-exports
 */
export type {
  FailedReservation,
  QuotaTokenRecord as RawQuotaTokenRecord,
} from "golem:quota/types@1.5.0"
