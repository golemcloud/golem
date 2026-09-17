/**
 * Test-case primitives shared by every per-agent module under
 * `test-infra/cases/`.
 *
 * A `TestCase` is just an Effect plus metadata. The runner loads all
 * cases, filters by name, then provides each one with the harness
 * services (`GolemCli`, `TestSession`).
 */
import { Context, Data, Effect } from "effect"
import { GolemCli, GolemCliError } from "./golem-cli.ts"

export class TestFailure extends Data.TaggedError("TestFailure")<{
  readonly testName: string
  readonly message: string
  readonly diagnostic?: string
}> {
  override get message(): string {
    return `[${this.testName}] ${this.message}` + (this.diagnostic ? `\n${this.diagnostic}` : "")
  }
}

export interface TestSessionState {
  /** Unique stamp shared by every test in one run; appended to agent
   * instance names so reruns do not collide. */
  readonly stamp: string
  /** The test currently running — used inside assertion helpers to
   * embed a useful name in `TestFailure`. */
  readonly currentTest: string
}

export class TestSession extends Context.Service<TestSession, TestSessionState>()("TestSession") {}

/** Strict equality. */
export const expectEqual = <A>(
  actual: A,
  expected: A,
  description: string,
): Effect.Effect<void, TestFailure, TestSession> =>
  Effect.gen(function* () {
    if (actual !== expected) {
      const session = yield* TestSession
      return yield* Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: `${description}: expected ${JSON.stringify(expected)} but got ${JSON.stringify(actual)}`,
        }),
      )
    }
  })

/** Substring match against arbitrary text. */
export const expectContains = (
  haystack: string,
  needle: string,
  description: string,
): Effect.Effect<void, TestFailure, TestSession> =>
  Effect.gen(function* () {
    if (!haystack.includes(needle)) {
      const session = yield* TestSession
      return yield* Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: `${description}: expected output to contain ${JSON.stringify(needle)}`,
          diagnostic: haystack.length > 4000 ? haystack.slice(-4000) : haystack,
        }),
      )
    }
  })

/** Regex match against arbitrary text. */
export const expectMatch = (
  haystack: string,
  pattern: RegExp,
  description: string,
): Effect.Effect<void, TestFailure, TestSession> =>
  Effect.gen(function* () {
    if (!pattern.test(haystack)) {
      const session = yield* TestSession
      return yield* Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: `${description}: expected output to match ${pattern}`,
          diagnostic: haystack.length > 4000 ? haystack.slice(-4000) : haystack,
        }),
      )
    }
  })

/** Assert that an `invoke` call FAILS at the host level (non-zero
 * exit code). Returns the captured CLI result so the caller can
 * additionally pattern-match on stderr. */
export const expectInvokeFails = (
  ref: string,
  method: string,
  args: ReadonlyArray<string> = [],
): Effect.Effect<
  { stdout: string; stderr: string; exitCode: number },
  TestFailure | GolemCliError,
  GolemCli | TestSession
> =>
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const res = yield* cli.invoke(ref, method, args, { allowFail: true })
    if (res.exitCode === 0) {
      return yield* Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: `expected invoke ${ref}.${method} to fail but it succeeded`,
          diagnostic: res.stdout,
        }),
      )
    }
    return res
  })

/** Map any GolemCliError into a TestFailure tied to the current test. */
export const liftCliError = <A, R>(
  effect: Effect.Effect<A, GolemCliError, R>,
): Effect.Effect<A, TestFailure, R | TestSession> =>
  effect.pipe(
    Effect.catchTag("GolemCliError", (e) =>
      Effect.gen(function* () {
        const session = yield* TestSession
        return yield* Effect.fail(
          new TestFailure({
            testName: session.currentTest,
            message: `golem CLI failed: ${e.command.join(" ")}`,
            diagnostic: `exit=${e.exitCode}\nstdout:\n${e.stdout}\nstderr:\n${e.stderr}`,
          }),
        )
      }),
    ),
  )

/** Run `golem agent update --await ref mode`, but tolerate "Worker is
 *  already at the target version" — that just means the worker was
 *  created at the latest revision (typical on a fresh deploy where
 *  the integration-test only deploys once), so there is nothing to
 *  update *to*. The semantic property the test wants — "state
 *  survives an update step" — is trivially true at the same revision.
 */
export const updateTolerant = (
  ref: string,
  mode: "manual" | "auto" = "manual",
): Effect.Effect<void, TestFailure, GolemCli | TestSession> =>
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const res = yield* cli
      .run(["--local", "--yes", "agent", "update", "--await", ref, mode], { allowFail: true })
      .pipe(
        Effect.catchTag("GolemCliError", (e) =>
          Effect.succeed({
            stdout: e.stdout,
            stderr: e.stderr,
            exitCode: e.exitCode,
          }),
        ),
      )
    if (res.exitCode === 0) return
    const blob = `${res.stdout}\n${res.stderr}`
    if (/already at the target version/i.test(blob)) {
      return
    }
    const session = yield* TestSession
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: `golem agent update --await ${ref} ${mode} failed`,
        diagnostic: `exit=${res.exitCode}\nstdout:\n${res.stdout}\nstderr:\n${res.stderr}`,
      }),
    )
  })

export interface TestCase {
  readonly name: string
  readonly description: string
  readonly run: Effect.Effect<void, TestFailure | GolemCliError, GolemCli | TestSession>
}

/** Helper to wrap a body Effect with the test's name in TestSession. */
export const defineCase = (
  name: string,
  description: string,
  run: Effect.Effect<void, TestFailure | GolemCliError, GolemCli | TestSession>,
): TestCase => ({ name, description, run })
