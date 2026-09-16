/**
 * Shared "drive a counter agent" body, used by all RDBMS-counter
 * cases (PgCounter, MySqlCounter, IgniteCounter) plus
 * SqliteCounter. Mirrors the original `run-rdbms-tests.mjs` matrix:
 *
 *   1. value / add / transferAdd / failingAdd / streamAll matrix
 *   2. ten more `add` invocations to trigger the everyN(10) snapshot
 *   3. oplog scan asserts a SNAPSHOT entry was emitted
 *   4. update --await + post-update value to confirm state preserved
 *
 * SqliteCounter has a slightly different surface (no transferAdd /
 * failingAdd / streamAll); a separate, smaller helper drives it.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import {
  TestFailure,
  TestSession,
  expectContains,
  expectMatch,
  liftCliError,
  updateTolerant,
} from "../harness/case.ts"

export interface RdbmsCounterDriver {
  /** "PgCounter" | "MySqlCounter" | "IgniteCounter" */
  readonly agentName: string
  /** Short prefix used in the unique instance name. */
  readonly slug: string
  /**
   * Override the post-matrix value regex. Defaults to `\b(8|10)\b`
   * (Postgres / MySQL roll back `failingAdd(100)` cleanly, leaving
   * the counter at 8 or 10). Apache Ignite 2.x's H2-backed SQL
   * engine does NOT roll back DML inside transactions even with
   * `IGNITE_ALLOW_DML_INSIDE_TRANSACTION=true`, so its expected
   * value includes the non-rolled-back state (= 110).
   */
  readonly afterMatrixValuePattern?: RegExp
}

const ref = (agentName: string, instance: string) => `${agentName}("${instance}")`

export const driveRdbmsCounter = ({
  agentName,
  slug,
  afterMatrixValuePattern = /\b(8|10)\b/,
}: RdbmsCounterDriver): Effect.Effect<void, TestFailure, GolemCli | TestSession> =>
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const counterName = `${slug}-${session.stamp}`
    const r = ref(agentName, counterName)

    // Matrix.
    yield* liftCliError(cli.invoke(r, "value"))
    yield* liftCliError(cli.invoke(r, "add", ["5"]))
    yield* liftCliError(cli.invoke(r, "add", ["3"]))
    yield* liftCliError(cli.invoke(r, "transferAdd", [`"other-${counterName}"`, "2"]))
    yield* liftCliError(cli.invoke(r, "failingAdd", ["100"]))
    const valRes = yield* liftCliError(cli.invoke(r, "value"))
    yield* expectMatch(
      valRes.stdout,
      afterMatrixValuePattern,
      `${agentName}.value after add(5)+add(3)+transferAdd(2) (rolled-back failingAdd)`,
    )
    yield* liftCliError(cli.invoke(r, "streamAll"))

    // Snapshot drill.
    for (let i = 0; i < 10; i++) {
      yield* liftCliError(cli.invoke(r, "add", ["1"]))
    }

    // Oplog must contain a SNAPSHOT entry.
    const oplog = yield* liftCliError(cli.oplog(r))
    yield* expectMatch(oplog.stdout, /SNAPSHOT/i, `${agentName} oplog has SNAPSHOT entry`)

    // update --await — must not error; state must survive.
    yield* updateTolerant(r, "manual")
    const afterUpdate = yield* liftCliError(cli.invoke(r, "value"))
    yield* expectContains(afterUpdate.stdout, "1", `${agentName}.value after update`)
  })

export const driveSqliteCounter = (): Effect.Effect<void, TestFailure, GolemCli | TestSession> =>
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const instance = `sqlite-${session.stamp}`
    const r = ref("SqliteCounter", instance)

    yield* liftCliError(cli.invoke(r, "value"))
    yield* liftCliError(cli.invoke(r, "increment"))
    yield* liftCliError(cli.invoke(r, "add", ["5"]))
    const valRes = yield* liftCliError(cli.invoke(r, "value"))
    yield* expectMatch(valRes.stdout, /\b6\b/, "SqliteCounter.value after increment+add(5)")

    // Trigger the everyN(10) snapshot policy: invoke 10 more times so
    // the multipart/mixed envelope (with the SQLite db blob) gets
    // emitted as a SNAPSHOT entry in the oplog.
    for (let i = 0; i < 10; i++) {
      yield* liftCliError(cli.invoke(r, "add", ["1"]))
    }

    // Oplog must contain a SNAPSHOT entry. The auto-snapshot save
    // path in `dispatchSaveSnapshot` is intentionally synchronous;
    // when it was async (returning a Promise from JS) the host
    // trapped with `wasm trap: cannot enter component instance`
    // every Nth invocation and the SNAPSHOT entry was never
    // recorded. This assertion is the regression guard for that
    // fix; see the long comment on `dispatchSaveSnapshot` in
    // `src/agent.ts` for what we know empirically about the trap.
    const oplog = yield* liftCliError(cli.oplog(r))
    yield* expectMatch(oplog.stdout, /SNAPSHOT/i, "SqliteCounter oplog has SNAPSHOT entry")

    yield* updateTolerant(r, "manual")
    const afterUpdate = yield* liftCliError(cli.invoke(r, "value"))
    yield* expectMatch(
      afterUpdate.stdout,
      /\b16\b/,
      "SqliteCounter.value after update preserves state (6 + 10×add(1) = 16)",
    )
  })
