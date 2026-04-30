/**
 * Test-case orchestrator. Iterates through the registry, runs each
 * case in isolation (so one failure does not abort the suite), and
 * reports a final pass/fail count to the user.
 */
import { Console, Duration, Effect, Exit } from "effect"
import { GolemCli } from "./golem-cli.ts"
import { TestCase, TestFailure, TestSession } from "./case.ts"

export interface RunOptions {
  readonly filter?: RegExp
  readonly stopOnFirstFailure?: boolean
}

export interface CaseOutcome {
  readonly name: string
  readonly description: string
  readonly status: "passed" | "failed" | "skipped"
  readonly durationMillis: number
  readonly failure?: TestFailure
  readonly cliFailure?: {
    command: ReadonlyArray<string>
    exitCode: number
    stdout: string
    stderr: string
  }
}

const formatLine = (o: CaseOutcome): string => {
  const dot = o.status === "passed" ? "✓" : o.status === "failed" ? "✗" : "·"
  const dur = `${o.durationMillis} ms`
  return `  ${dot} ${o.name}  (${dur})  — ${o.description}`
}

export const runCases = (
  cases: ReadonlyArray<TestCase>,
  opts: RunOptions = {},
): Effect.Effect<ReadonlyArray<CaseOutcome>, never, GolemCli> =>
  Effect.gen(function* () {
    const filtered = opts.filter ? cases.filter((c) => opts.filter!.test(c.name)) : cases
    const stamp = `${Date.now().toString(36)}`
    yield* Console.log(
      `Running ${filtered.length} test case${filtered.length === 1 ? "" : "s"} (stamp=${stamp})`,
    )

    const outcomes: Array<CaseOutcome> = []
    for (const tc of filtered) {
      yield* Console.log(`\n--- ${tc.name} ---`)
      const start = Date.now()
      const exit = yield* tc.run.pipe(
        Effect.provideService(TestSession, { stamp, currentTest: tc.name }),
        Effect.timeout(Duration.minutes(10)),
        Effect.exit,
      )
      const durationMillis = Date.now() - start
      const outcome: CaseOutcome = (() => {
        if (Exit.isSuccess(exit)) {
          return {
            name: tc.name,
            description: tc.description,
            status: "passed",
            durationMillis,
          }
        }
        const cause = exit.cause
        // Extract the first typed failure if any.
        const failures: Array<TestFailure | unknown> = []
        const walk = (c: unknown): void => {
          const co = c as { _tag?: string; left?: unknown; right?: unknown; failure?: unknown }
          if (co?._tag === "Fail") failures.push(co.failure)
          else if (co?._tag === "Sequential" || co?._tag === "Parallel") {
            walk(co.left)
            walk(co.right)
          } else if (co?._tag === "Die") failures.push(co.failure)
        }
        walk(cause)
        const first = failures[0]
        if (first instanceof TestFailure) {
          return {
            name: tc.name,
            description: tc.description,
            status: "failed",
            durationMillis,
            failure: first,
          }
        }
        if (
          first &&
          typeof first === "object" &&
          (first as { _tag?: string })._tag === "GolemCliError"
        ) {
          const e = first as {
            command: ReadonlyArray<string>
            exitCode: number
            stdout: string
            stderr: string
          }
          return {
            name: tc.name,
            description: tc.description,
            status: "failed",
            durationMillis,
            cliFailure: {
              command: e.command,
              exitCode: e.exitCode,
              stdout: e.stdout,
              stderr: e.stderr,
            },
          }
        }
        return {
          name: tc.name,
          description: tc.description,
          status: "failed",
          durationMillis,
          failure: new TestFailure({
            testName: tc.name,
            message: "unhandled failure in test",
            diagnostic: JSON.stringify(first ?? cause, null, 2),
          }),
        }
      })()
      outcomes.push(outcome)
      yield* Console.log(formatLine(outcome))
      if (outcome.failure) {
        yield* Console.error(`    ${outcome.failure.message}`)
        if (outcome.failure.diagnostic) {
          yield* Console.error(`    ${outcome.failure.diagnostic.split("\n").join("\n    ")}`)
        }
      }
      if (outcome.cliFailure) {
        yield* Console.error(
          `    golem ${outcome.cliFailure.command.join(" ")} → exit ${outcome.cliFailure.exitCode}`,
        )
        if (outcome.cliFailure.stderr) {
          yield* Console.error(
            `    stderr: ${outcome.cliFailure.stderr.split("\n").slice(-20).join("\n    stderr: ")}`,
          )
        }
      }
      if (outcome.status === "failed" && opts.stopOnFirstFailure) {
        break
      }
    }

    return outcomes
  })

export const summarize = (
  outcomes: ReadonlyArray<CaseOutcome>,
): Effect.Effect<{ readonly total: number; readonly passed: number; readonly failed: number }> =>
  Effect.sync(() => {
    const passed = outcomes.filter((o) => o.status === "passed").length
    const failed = outcomes.filter((o) => o.status === "failed").length
    return { total: outcomes.length, passed, failed }
  })
