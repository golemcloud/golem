/** Finite guest-side saga retries against the real runtime. */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const okRef = `InventorySaga("inv-ok-${session.stamp}")`
  const retryRef = `InventorySaga("inv-retry-${session.stamp}")`
  const ok = yield* liftCliError(cli.invoke(okRef, "runOnceOk"))
  yield* expectMatch(ok.stdout, /reserved[^0-9]+7/, "runOnceOk reserved=7")
  yield* expectMatch(ok.stdout, /committed[^0-9]+7/, "runOnceOk committed=7")
  yield* expectMatch(ok.stdout, /totalAttempts[^0-9]+1/, "runOnceOk totalAttempts=1")

  const retried = yield* liftCliError(cli.invoke(retryRef, "runWithRetries"))
  yield* expectMatch(retried.stdout, /totalAttempts[^0-9]+3/, "three successive saga attempts")
  const oplog = yield* liftCliError(cli.oplog(retryRef))
  yield* expectMatch(oplog.stdout, /reserve.comp/, "failed attempt records reserve compensation")
  yield* expectMatch(oplog.stdout, /commit.comp/, "failed attempt records commit compensation")
  if (/JUMP|BEGIN\s*ATOMIC\s*REGION|END\s*ATOMIC\s*REGION/i.test(oplog.stdout)) {
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: "saga retries must append calls without rewinding or atomic regions",
        diagnostic: oplog.stdout,
      }),
    )
  }
})

export const case_ = defineCase(
  "inventory-saga",
  "InventorySaga: compensation and guest-side retries",
  run,
)
