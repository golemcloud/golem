/**
 * BookingSaga case — exercises `Saga.fallibleTransaction` end-to-end.
 *
 *   1. Happy path → tag = "ok", trace contains every step's exec line
 *      and no compensation lines.
 *   2. Force flight failure → tag = "failed-completely". Flight comp
 *      is the only compensation that should fire (later steps never
 *      ran).
 *   3. Force hotel failure with `compFailsAt: "hotel"` → tag =
 *      "failed-partially". Flight + hotel compensations both run; the
 *      hotel comp itself failed and is reported in compensationError.
 *   4. After the three drives, oplog must contain balanced
 *      `BeginAtomicRegion` / `EndAtomicRegion` entries.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `booking-${session.stamp}`
  const r = `BookingSaga("${name}")`

  // 1. Happy path.
  const ok = yield* liftCliError(cli.invoke(r, "book", ["null", "null"]))
  yield* expectMatch(ok.stdout, /tag[^a-zA-Z]+ok/, "happy path: tag = ok")
  yield* expectMatch(ok.stdout, /exec:flight/, "happy path trace records flight exec")
  yield* expectMatch(ok.stdout, /exec:hotel/, "happy path trace records hotel exec")
  yield* expectMatch(ok.stdout, /exec:car/, "happy path trace records car exec")
  if (/comp:/.test(ok.stdout)) {
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: "happy path should not run any compensations",
        diagnostic: ok.stdout,
      }),
    )
  }

  // 2. Force flight failure → FailedAndRolledBackCompletely.
  const flightFail = yield* liftCliError(cli.invoke(r, "book", [`"flight"`, "null"]))
  yield* expectMatch(
    flightFail.stdout,
    /failed-completely/,
    "flight failure: tag = failed-completely",
  )
  yield* expectMatch(
    flightFail.stdout,
    /booking-failed:flight/,
    "flight failure error includes booking-failed:flight",
  )
  // Flight failed before producing a value, so its compensation
  // should NOT have run (operations only register comp on success).
  if (/exec:hotel/.test(flightFail.stdout) || /exec:car/.test(flightFail.stdout)) {
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: "flight failure should short-circuit before hotel/car",
        diagnostic: flightFail.stdout,
      }),
    )
  }

  // 3. Car fails after hotel succeeded with a failing comp →
  //    FailedAndRolledBackPartially. (For partial rollback to fire
  //    the failing step's comp must have been *registered* — i.e.
  //    the step succeeded — and then a later step must fail.)
  const partial = yield* liftCliError(cli.invoke(r, "book", [`"car"`, `"hotel"`]))
  yield* expectMatch(
    partial.stdout,
    /failed-partially/,
    "car failure with failing hotel-comp: tag = failed-partially",
  )
  yield* expectMatch(
    partial.stdout,
    /booking-failed:car/,
    "partial failure error contains booking-failed:car",
  )
  yield* expectMatch(
    partial.stdout,
    /hotel-comp-failed/,
    "partial failure compensationError contains hotel-comp-failed",
  )
  yield* expectMatch(
    partial.stdout,
    /comp:flight/,
    "partial failure trace records flight comp (reverse-order drain)",
  )
  yield* expectMatch(
    partial.stdout,
    /comp:hotel/,
    "partial failure trace records hotel comp (the failing one)",
  )

  // 4. Oplog must contain matched atomic-region pairs.
  const oplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(
    oplog.stdout,
    /BEGIN\s*ATOMIC\s*REGION/i,
    "BookingSaga oplog has BeginAtomicRegion entry",
  )
  yield* expectMatch(
    oplog.stdout,
    /END\s*ATOMIC\s*REGION/i,
    "BookingSaga oplog has EndAtomicRegion entry",
  )
})

export const case_ = defineCase(
  "booking-saga",
  "BookingSaga: fallible transaction (ok / FailedAndRolledBackCompletely / FailedAndRolledBackPartially)",
  run,
)
