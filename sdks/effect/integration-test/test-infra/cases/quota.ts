/**
 * QuotaTester case — exercises every entry point on the Effect-typed
 * `Quota.*` namespace against a real `effect-golem-test-quota`
 * resource (Capacity = 10, reject enforcement; see golem.yaml
 * `resourceDefaults.local`).
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `quota-${session.stamp}`
  const r = `QuotaTester("${name}")`

  // Acquire a token; the host echoes back resourceName + expectedUse.
  const acquired = yield* liftCliError(cli.invoke(r, "acquire", [`"3"`]))
  yield* expectMatch(
    acquired.stdout,
    /effect-golem-test-quota/,
    "Quota.acquireQuotaToken returns the resourceName",
  )
  yield* expectMatch(acquired.stdout, /\b3\b/, "Quota.acquireQuotaToken echoes the expectedUse")

  // withReservation happy path — body commits `used` tokens.
  const ok = yield* liftCliError(cli.invoke(r, "withReservationOk", [`"2"`, `"2"`]))
  yield* expectMatch(ok.stdout, /committed:2/, "withReservation happy path commits used")

  // withReservation with body Effect.fail — wrapper falls back to
  // commit(0) and surfaces the typed failure on the catch path.
  const fail = yield* liftCliError(cli.invoke(r, "withReservationFailure", [`"1"`]))
  yield* expectMatch(
    fail.stdout,
    /failed:body-said-no/,
    "withReservation failure path returns failed:body-said-no",
  )

  // Manual reserve + commit.
  const manualOk = yield* liftCliError(cli.invoke(r, "manualReserveCommit", [`"2"`, `"1"`]))
  yield* expectMatch(
    manualOk.stdout,
    /manual-commit:1/,
    "manualReserveCommit returns manual-commit:1",
  )

  // Manual reserve + drop (no explicit commit) → host treats as commit(0).
  const manualDrop = yield* liftCliError(cli.invoke(r, "manualReserveDrop", [`"1"`]))
  yield* expectMatch(
    manualDrop.stdout,
    /manual-drop:0/,
    "manualReserveDrop returns manual-drop:0 (drop ≡ commit(0))",
  )

  // Split + merge round-trip.
  const split = yield* liftCliError(cli.invoke(r, "splitMerge", [`"5"`, `"2"`]))
  yield* expectMatch(
    split.stdout,
    /afterSplitParent[^0-9]+3/,
    "Quota.split: parent expectedUse drops to 3 (5 - 2)",
  )
  yield* expectMatch(split.stdout, /afterSplitChild[^0-9]+2/, "Quota.split: child expectedUse = 2")
  yield* expectMatch(
    split.stdout,
    /afterMerge[^0-9]+5/,
    "Quota.merge: parent expectedUse restored to 5",
  )

  // Exhaust-and-reject: ask for 11 against a Capacity=10 resource;
  // host's reject enforcement returns FailedReservationError.
  const rejected = yield* liftCliError(cli.invoke(r, "exhaustAndReject", [`"11"`]))
  yield* expectMatch(
    rejected.stdout,
    /rejected/,
    "exhaustAndReject(11) on Capacity=10 reject-resource → FailedReservationError",
  )
})

export const case_ = defineCase(
  "quota",
  "QuotaTester: full Quota.* surface (acquire / reserve / commit / split / merge / reject)",
  run,
)
