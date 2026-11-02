/**
 * PgCounter case — drives the Postgres-backed counter agent through
 * the standard matrix + snapshot + update drill.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectEqual, liftCliError } from "../harness/case.ts"
import { driveRdbmsCounter } from "./_counter-helper.ts"

export const case_ = defineCase(
  "pg-counter",
  "PgCounter (Postgres-backed counter): matrix + snapshot + update drill",
  driveRdbmsCounter({ agentName: "PgCounter", slug: "pg" }),
)

export const migrationsCase = defineCase(
  "pg-migrations",
  "Effect Postgres Migrator applies migrations once using ordinary table grants",
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const ref = `PgCounter("migration-${session.stamp}")`
    for (const applied of [1, 0]) {
      const output = yield* liftCliError(
        cli.run(["--local", "--format", "json", "agent", "invoke", "--no-stream", ref, "migrate"]),
      )
      const result = yield* Effect.try({
        try: () => JSON.parse(output.stdout),
        catch: () =>
          new TestFailure({
            testName: session.currentTest,
            message: "Migration invocation returned invalid JSON",
            diagnostic: output.stdout,
          }),
      })
      yield* expectEqual(
        result.resultJson.value.value,
        `applied=${applied},rows=1`,
        "pending migrations applied and migration data persisted",
      )
    }
  }),
)
