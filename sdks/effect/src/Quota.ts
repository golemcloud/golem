import { Cause, Effect, Exit, Schema, SchemaGetter, Scope } from "effect"
import type * as QuotaHost from "golem:quota/types@1.5.0"
import type { QuotaToken as RawQuotaToken } from "golem:core/types@2.0.0"
import { QuotaClient } from "./host/QuotaClient.js"
import { assertCapabilityReady } from "./internal/schema-model/capabilityTransaction.js"
import { QUOTA_INTERNAL } from "./internal/schema-model/quotaInternal.js"
import {
  createGuestQuotaTokenHandle,
  GuestQuotaTokenHandle,
  peekGuestQuotaTokenHandle,
  takeGuestQuotaTokenHandle,
} from "./internal/schema-model/quotaTokenHandle.js"
import { witQuotaTokenAnnotationKey } from "./WitTypes.js"

const handles = new WeakMap<QuotaToken, GuestQuotaTokenHandle>()
const handleOf = (token: QuotaToken): GuestQuotaTokenHandle => {
  const handle = handles.get(token)
  if (handle === undefined) throw new Error("invalid quota token")
  return handle
}
const rawOf = (token: QuotaToken): RawQuotaToken => {
  assertCapabilityReady(handleOf(token))
  const raw = peekGuestQuotaTokenHandle(QUOTA_INTERNAL, handleOf(token))
  if (raw === undefined) throw new Error("quota token has already been transferred")
  return raw
}
const wrapHandle = (handle: GuestQuotaTokenHandle): QuotaToken => {
  const token = Object.create(QuotaToken.prototype) as QuotaToken
  handles.set(token, handle)
  return token
}
const wrap = (raw: RawQuotaToken): QuotaToken =>
  wrapHandle(createGuestQuotaTokenHandle(QUOTA_INTERNAL, raw))

export class QuotaToken {
  private constructor() {}
  toJSON(): never {
    throw new Error("quota tokens cannot be serialized; transfer them through a schema value")
  }
}

const QuotaTokenHandleSchema = Schema.declare(
  (u): u is GuestQuotaTokenHandle => u instanceof GuestQuotaTokenHandle,
).pipe(Schema.annotate({ [witQuotaTokenAnnotationKey]: true }))

export const QuotaTokenSchema: Schema.Codec<QuotaToken, GuestQuotaTokenHandle> =
  QuotaTokenHandleSchema.pipe(
    Schema.decodeTo(
      Schema.declare((u): u is QuotaToken => u instanceof QuotaToken),
      {
        decode: SchemaGetter.transform(wrapHandle),
        encode: SchemaGetter.transform(handleOf),
      },
    ),
  )

export interface Reservation {
  readonly [ReservationTypeId]: typeof ReservationTypeId
}
export const ReservationTypeId: unique symbol = Symbol.for("@effect-golem/quota/Reservation")
export type ReservationTypeId = typeof ReservationTypeId
interface ReservationImpl extends Reservation {
  readonly raw: QuotaHost.Reservation
  committed: boolean
}
const reservation = (raw: QuotaHost.Reservation): ReservationImpl => ({
  [ReservationTypeId]: ReservationTypeId,
  raw,
  committed: false,
})

export class FailedReservationError {
  readonly _tag = "FailedReservationError"
  readonly estimatedWaitNanos: bigint | undefined
  readonly message: string
  constructor(raw: QuotaHost.FailedReservation) {
    this.estimatedWaitNanos = raw.estimatedWaitNanos
    this.message = `FailedReservationError${raw.estimatedWaitNanos === undefined ? "" : ` (${raw.estimatedWaitNanos}ns)`}`
  }
  toJSON() {
    return { _tag: this._tag, message: this.message }
  }
}
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
const isFailed = (u: unknown): u is QuotaHost.FailedReservation =>
  typeof u === "object" && u !== null && "estimatedWaitNanos" in u

export const acquireQuotaToken = (
  resourceName: string,
  expectedUse: bigint,
): Effect.Effect<QuotaToken, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const host = yield* QuotaClient
    return yield* Effect.try({
      try: () => wrap(host.newToken(resourceName, expectedUse)),
      catch: (cause) => new QuotaHostError("newToken", cause),
    })
  })

export const reserve = (
  token: QuotaToken,
  amount: bigint,
): Effect.Effect<Reservation, FailedReservationError | QuotaHostError, Scope.Scope | QuotaClient> =>
  Effect.gen(function* () {
    const host = yield* QuotaClient
    return yield* Effect.acquireRelease(
      Effect.try({
        try: () => reservation(host.reserve(rawOf(token), amount)),
        catch: (cause) =>
          isFailed(cause)
            ? new FailedReservationError(cause)
            : new QuotaHostError("reserve", cause),
      }),
      (value) =>
        Effect.sync(() => {
          if (!value.committed) {
            try {
              host.commit(value.raw, 0n)
            } catch {
              /* best-effort resource drop */
            }
            value.committed = true
          }
        }),
    )
  })

export const commit = (
  value: Reservation,
  used: bigint,
): Effect.Effect<void, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const r = value as ReservationImpl
    if (r.committed)
      return yield* Effect.fail(new QuotaHostError("commit", new Error("already committed")))
    const host = yield* QuotaClient
    yield* Effect.try({
      try: () => host.commit(r.raw, used),
      catch: (cause) => new QuotaHostError("commit", cause),
    })
    r.committed = true
  })

export const split = (
  token: QuotaToken,
  expectedUse: bigint,
): Effect.Effect<QuotaToken, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    const host = yield* QuotaClient
    return yield* Effect.try({
      try: () => wrap(host.split(rawOf(token), expectedUse)),
      catch: (cause) => new QuotaHostError("split", cause),
    })
  })

export const merge = (
  token: QuotaToken,
  other: QuotaToken,
): Effect.Effect<void, QuotaHostError, QuotaClient> =>
  Effect.gen(function* () {
    if (token === other)
      return yield* Effect.fail(
        new QuotaHostError("merge", new Error("cannot merge token with itself")),
      )
    const host = yield* QuotaClient
    yield* Effect.try({
      try: () => {
        const target = rawOf(token)
        const source = rawOf(other)
        takeGuestQuotaTokenHandle(QUOTA_INTERNAL, handleOf(other))
        host.merge(target, source)
      },
      catch: (cause) => new QuotaHostError("merge", cause),
    })
  })

export const withReservation = <A, E, R>(
  token: QuotaToken,
  amount: bigint,
  body: (reservation: Reservation) => Effect.Effect<{ used: bigint; value: A }, E, R>,
): Effect.Effect<A, E | FailedReservationError | QuotaHostError, R | QuotaClient> =>
  Effect.scoped(
    Effect.gen(function* () {
      const r = yield* reserve(token, amount)
      const exit = yield* Effect.exit(body(r))
      if (Exit.isSuccess(exit)) {
        yield* commit(r, exit.value.used)
        return exit.value.value
      }
      return yield* Effect.failCause(exit.cause as Cause.Cause<E | QuotaHostError>)
    }),
  )

export type { FailedReservation } from "golem:quota/types@1.5.0"
