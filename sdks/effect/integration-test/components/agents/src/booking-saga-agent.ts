/**
 * BookingSaga — exercises `Saga.fallibleTransaction` end-to-end inside
 * a real Golem runtime.
 *
 * The agent simulates a 3-step booking workflow (flight + hotel + car)
 * using two `Saga.operation`s with infallible compensations and one
 * `Saga.withFallibleCompensation` step whose rollback can be made to
 * fail on demand. Each step writes to an in-memory log so callers can
 * inspect the execution trace; the saga also threads through the
 * Effect log + tracer wiring so the host oplog shows
 * `BeginAtomicRegion` / `EndAtomicRegion` markers around each step.
 *
 * Methods:
 *
 * - `book({ shouldFailAt, compFailsAt })` — runs the saga. Pass
 *   `shouldFailAt: "flight" | "hotel" | "car" | null` to force a typed
 *   failure at the named step. Pass `compFailsAt: "hotel" | null` to
 *   force the (fallible) hotel compensation to itself fail. The
 *   returned object reports `tag: "ok" | "failed-completely" |
 *   "failed-partially"` plus the recorded execution trace.
 * - `trace()` — returns the trace recorded by the most recent `book`
 *   invocation (or an empty array on first call).
 *
 * Snapshotting is enabled with a small empty-state Ref so the host
 * exercises the snapshot oplog entry alongside the saga drill.
 */
import { Cause, Effect, Exit, Ref, Schema } from "effect"
import { defineAgent, method, Saga, Snapshot } from "effect-golem"

const StepSchema = Schema.Literals(["flight", "hotel", "car"])

const BookOutcome = Schema.Struct({
  tag: Schema.Literals(["ok", "failed-completely", "failed-partially"]),
  trace: Schema.Array(Schema.String),
  error: Schema.optional(Schema.String),
  compensationError: Schema.optional(Schema.String),
})

type BookOk = { tag: "ok"; trace: ReadonlyArray<string> }
type BookFailedCompletely = {
  tag: "failed-completely"
  trace: ReadonlyArray<string>
  error: string
}
type BookFailedPartially = {
  tag: "failed-partially"
  trace: ReadonlyArray<string>
  error: string
  compensationError: string
}
type BookResult = BookOk | BookFailedCompletely | BookFailedPartially

export const BookingSaga = defineAgent({
  name: "BookingSaga",
  description: "Three-step booking saga that exercises Saga.fallibleTransaction end-to-end",
  mode: "durable",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    book: method({
      params: {
        shouldFailAt: Schema.NullOr(StepSchema),
        compFailsAt: Schema.NullOr(Schema.Literal("hotel")),
      },
      success: BookOutcome,
      description:
        "Runs the booking saga. Optional shouldFailAt forces a typed failure at the named step.",
    }),
    trace: method({
      params: {},
      success: Schema.Array(Schema.String),
      description: "Returns the trace recorded by the most recent book() call.",
    }),
  },
}).implement((_params, snap) =>
  Effect.gen(function* () {
    yield* snap.init({})
    const traceRef = yield* Ref.make<Array<string>>([])

    const log = (line: string) =>
      Ref.update(traceRef, (xs) => [...xs, line]).pipe(Effect.tap(() => Effect.logInfo(line)))

    const flightOp = Saga.operation({
      execute: ({ name, fail }: { name: string; fail: boolean }) =>
        Effect.gen(function* () {
          yield* log(`exec:flight:${name}`)
          if (fail) return yield* Effect.fail(`booking-failed:flight` as const)
          return { ref: `FLT-${name}` }
        }),
      compensate: ({ name }: { name: string; fail: boolean }) => log(`comp:flight:${name}`),
    })

    const carOp = Saga.operation({
      execute: ({ name, fail }: { name: string; fail: boolean }) =>
        Effect.gen(function* () {
          yield* log(`exec:car:${name}`)
          if (fail) return yield* Effect.fail(`booking-failed:car` as const)
          return { ref: `CAR-${name}` }
        }),
      compensate: ({ name }: { name: string; fail: boolean }) => log(`comp:car:${name}`),
    })

    const bookHotel = (input: { name: string; fail: boolean; compShouldFail: boolean }) =>
      Saga.withFallibleCompensation(
        Effect.gen(function* () {
          yield* log(`exec:hotel:${input.name}`)
          if (input.fail) return yield* Effect.fail(`booking-failed:hotel` as const)
          return { ref: `HTL-${input.name}` }
        }),
        () =>
          Effect.gen(function* () {
            yield* log(`comp:hotel:${input.name}`)
            if (input.compShouldFail) {
              return yield* Effect.fail(`hotel-comp-failed:${input.name}` as const)
            }
          }),
      )

    return {
      book: ({ shouldFailAt, compFailsAt }) =>
        Effect.gen(function* () {
          yield* Ref.set(traceRef, [])
          const ownerName = "demo"
          const exit = yield* Effect.exit(
            Saga.fallibleTransaction(
              Effect.gen(function* () {
                yield* flightOp({ name: ownerName, fail: shouldFailAt === "flight" })
                yield* bookHotel({
                  name: ownerName,
                  fail: shouldFailAt === "hotel",
                  compShouldFail: compFailsAt === "hotel",
                })
                yield* carOp({ name: ownerName, fail: shouldFailAt === "car" })
                return "ok" as const
              }),
            ),
          )
          const trace = yield* Ref.get(traceRef)
          if (Exit.isSuccess(exit)) {
            const out: BookOk = { tag: "ok", trace }
            return out as BookResult
          }
          const fail = exit.cause.reasons.find(Cause.isFailReason)
          const f = fail?.error
          if (
            typeof f === "object" &&
            f !== null &&
            "_tag" in f &&
            (f._tag === "FailedAndRolledBackCompletely" ||
              f._tag === "FailedAndRolledBackPartially")
          ) {
            const failure = f as Saga.TransactionFailure<string>
            if (failure._tag === "FailedAndRolledBackPartially") {
              const out: BookFailedPartially = {
                tag: "failed-partially",
                trace,
                error: String(failure.error),
                compensationError: String(failure.compensationError),
              }
              return out as BookResult
            }
            const out: BookFailedCompletely = {
              tag: "failed-completely",
              trace,
              error: String(failure.error),
            }
            return out as BookResult
          }
          // Should never happen — surface as a defect so the host
          // sees the genuine root cause.
          return yield* Effect.die(exit.cause)
        }).pipe(Effect.withSpan("BookingSaga.book")),
      trace: () => Ref.get(traceRef).pipe(Effect.map((xs) => [...xs] as ReadonlyArray<string>)),
    }
  }),
)
