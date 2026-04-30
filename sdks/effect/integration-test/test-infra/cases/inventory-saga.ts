/**
 * InventorySaga case — exercises `Saga.infallibleTransaction`.
 *
 *   1. runOnceOk → happy path, body returns its value, totalAttempts
 *      goes from 0 → 1.
 *   2. runOnceAndJump → the body's `failingSettleStep` deliberately
 *      fails, triggering the rewind protocol. After comps drain,
 *      `setOplogIndex(checkpoint)` causes the host to replay from
 *      the start. Replays loop forever by design (the failure is
 *      deterministic), so the harness drives the invoke under a
 *      bounded timeout, then asserts the oplog contains a `JUMP` /
 *      atomic-region trail proving the rewind protocol fired.
 */
import { Duration, Effect, Fiber } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const okName = `inv-ok-${session.stamp}`
  const jumpName = `inv-jump-${session.stamp}`
  const okRef = `InventorySaga("${okName}")`
  const jumpRef = `InventorySaga("${jumpName}")`

  // ---- 1. Happy path ----
  const ok = yield* liftCliError(cli.invoke(okRef, "runOnceOk"))
  yield* expectMatch(ok.stdout, /reserved[^0-9]+7/, "runOnceOk reserved=7")
  yield* expectMatch(ok.stdout, /committed[^0-9]+7/, "runOnceOk committed=7")
  yield* expectMatch(ok.stdout, /totalAttempts[^0-9]+1/, "runOnceOk totalAttempts=1")

  // ---- 2. Rewind drill ----
  // runOnceAndJump loops forever by design, so we fork the invoke,
  // give the host enough time to emit at least one JUMP, then
  // interrupt the fiber (which closes the spawn scope and kills
  // the local CLI process — the agent itself keeps running on the
  // server, but `oplog` reads the journal independently).
  const jumpFiber = yield* Effect.forkChild(
    cli.invoke(jumpRef, "runOnceAndJump", [], { allowFail: true }).pipe(Effect.ignore),
  )
  yield* Effect.sleep(Duration.seconds(20))
  yield* Fiber.interrupt(jumpFiber)

  // The oplog should now contain at least one JUMP marker plus
  // balanced atomic regions (one pair per saga step that ran).
  const oplog = yield* liftCliError(cli.oplog(jumpRef))
  yield* expectMatch(
    oplog.stdout,
    /JUMP/i,
    "InventorySaga.runOnceAndJump produced a JUMP entry (set-oplog-index)",
  )
  yield* expectMatch(
    oplog.stdout,
    /BEGIN\s*ATOMIC\s*REGION/i,
    "rewind drill: BeginAtomicRegion appears",
  )
  yield* expectMatch(
    oplog.stdout,
    /END\s*ATOMIC\s*REGION/i,
    "rewind drill: EndAtomicRegion appears",
  )
})

export const case_ = defineCase(
  "inventory-saga",
  "InventorySaga: infallibleTransaction happy path + rewind/JUMP via setOplogIndex",
  run,
)
