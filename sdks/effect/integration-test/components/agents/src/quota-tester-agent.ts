/**
 * QuotaTester — exercises the `effect-golem` `Quota.*` Effect-typed
 * wrappers around `golem:quota/types@1.5.0` against a real Golem
 * runtime.
 *
 * The agent declares no per-instance state; each method is a small,
 * focused probe of one wrapper:
 *
 * - `acquire({ expected })` — `Quota.acquireQuotaToken` returns a host
 *   `QuotaToken`; we encode it via the `QuotaToken` schema codec and
 *   return its `expectedUse` and `resourceName` so the test harness
 *   can assert the constructor went through the host.
 * - `withReservationOk({ amount, used })` — happy path through
 *   `Quota.withReservation`. Returns the body's value (echoes `used`
 *   so we can assert the commit went through).
 * - `withReservationFailure({ amount })` — drives the body to
 *   `Effect.fail`; the wrapper falls back to `commit(0)` via the
 *   surrounding scope. Returns `"failed"` (and the typed cause
 *   propagates to the host).
 * - `manualReserveCommit({ amount, used })` — exercises the manual
 *   `reserve` / `commit` pair inside `Effect.scoped`.
 * - `manualReserveDrop({ amount })` — exercises the drop ≡ commit(0)
 *   path: enters the scope, never explicitly commits, and exits.
 * - `splitMerge({ initial, child })` — exercises `Quota.split` and
 *   `Quota.merge` on the same token.
 * - `exhaustAndReject({ amount })` — issues `withReservation` with an
 *   `amount` that exceeds the resource's remaining capacity. Under the
 *   `effect-golem-test-quota` resource (Capacity=10, Reject) this
 *   returns the typed `FailedReservationError` exposed as a `failed`
 *   discriminator with `estimatedWaitNanos`.
 */
import { Effect, Schema } from "effect"
import { defineAgent, method, Quota } from "effect-golem"

const RESOURCE_NAME = "effect-golem-test-quota"

/** Result schema for `exhaustAndReject` — discriminated by the `_tag`
 *  field so success and rejection cases share one return type
 *  (effect-golem's wit-codec requires `_tag` for variant dispatch). */
const ReserveOutcome = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("ok"), used: Schema.String }),
  Schema.Struct({
    _tag: Schema.Literal("rejected"),
    estimatedWaitNanos: Schema.optional(Schema.String),
  }),
])

export const QuotaTester = defineAgent({
  name: "QuotaTester",
  description:
    "Probe agent that exercises the effect-golem Quota.* wrappers against a real Golem runtime",
  mode: "durable",
  constructorParams: { name: Schema.String },
  methods: {
    acquire: method({
      params: { expected: Schema.String },
      success: Schema.Struct({
        resourceName: Schema.String,
        expectedUse: Schema.String,
      }),
    }),
    withReservationOk: method({
      params: { amount: Schema.String, used: Schema.String },
      success: Schema.String,
    }),
    withReservationFailure: method({
      params: { amount: Schema.String },
      success: Schema.String,
    }),
    manualReserveCommit: method({
      params: { amount: Schema.String, used: Schema.String },
      success: Schema.String,
    }),
    manualReserveDrop: method({
      params: { amount: Schema.String },
      success: Schema.String,
    }),
    splitMerge: method({
      params: { initial: Schema.String, child: Schema.String },
      success: Schema.Struct({
        afterSplitParent: Schema.String,
        afterSplitChild: Schema.String,
        afterMerge: Schema.String,
      }),
    }),
    exhaustAndReject: method({
      params: { amount: Schema.String },
      success: ReserveOutcome,
    }),
  },
}).implement(() =>
  Effect.gen(function* () {
    return {
      acquire: ({ expected }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, BigInt(expected))
          const rec = token.toRecord()
          return {
            resourceName: rec.resourceName,
            expectedUse: rec.expectedUse.toString(),
          }
        }),

      withReservationOk: ({ amount, used }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, 1n)
          return yield* Quota.withReservation(token, BigInt(amount), () =>
            Effect.succeed({ used: BigInt(used), value: `committed:${used}` }),
          )
        }),

      withReservationFailure: ({ amount }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, 1n)
          return yield* Quota.withReservation(token, BigInt(amount), () =>
            Effect.gen(function* () {
              yield* Effect.fail("body-said-no" as const)
              return { used: 0n, value: "unreachable" }
            }),
          ).pipe(Effect.catch((e) => Effect.succeed(`failed:${String(e)}`)))
        }),

      manualReserveCommit: ({ amount, used }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, 1n)
          yield* Effect.scoped(
            Effect.gen(function* () {
              const reservation = yield* Quota.reserve(token, BigInt(amount))
              yield* Quota.commit(reservation, BigInt(used))
            }),
          )
          return `manual-commit:${used}`
        }),

      manualReserveDrop: ({ amount }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, 1n)
          yield* Effect.scoped(
            Effect.gen(function* () {
              yield* Quota.reserve(token, BigInt(amount))
              // No explicit commit — scope close ≡ host commit(0).
            }),
          )
          return "manual-drop:0"
        }),

      splitMerge: ({ initial, child }) =>
        Effect.gen(function* () {
          const parent = yield* Quota.acquireQuotaToken(RESOURCE_NAME, BigInt(initial))
          const childToken = yield* Quota.split(parent, BigInt(child))
          const afterSplitParent = parent.toRecord().expectedUse
          const afterSplitChild = childToken.toRecord().expectedUse
          yield* Quota.merge(parent, childToken)
          const afterMerge = parent.toRecord().expectedUse
          return {
            afterSplitParent: afterSplitParent.toString(),
            afterSplitChild: afterSplitChild.toString(),
            afterMerge: afterMerge.toString(),
          }
        }),

      exhaustAndReject: ({ amount }) =>
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken(RESOURCE_NAME, 1n)
          return yield* Quota.withReservation(token, BigInt(amount), () =>
            Effect.succeed({
              used: BigInt(amount),
              value: { _tag: "ok" as const, used: amount },
            }),
          ).pipe(
            Effect.catchTag("FailedReservationError", (e) =>
              Effect.succeed({
                _tag: "rejected" as const,
                estimatedWaitNanos: e.estimatedWaitNanos?.toString(),
              }),
            ),
          )
        }),
    }
  }),
)
