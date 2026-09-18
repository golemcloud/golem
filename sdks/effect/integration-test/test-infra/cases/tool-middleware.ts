import { Effect } from "effect"
import {
  defineCase,
  expectContains,
  liftCliError,
  TestFailure,
  TestSession,
} from "../harness/case.ts"
import { GolemCli } from "../harness/golem-cli.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `tool-${session.stamp}`
  const payload = `stream-${session.stamp}`
  const result = yield* liftCliError(
    cli.invoke(`EffectToolCaller("${name}")`, "roundtrip", [`"${payload}"`]),
  )
  yield* expectContains(result.stdout, `accepted:${name}`, "typed Effect tool result")
  yield* expectContains(
    result.stdout,
    `tool:${name}:${payload.toUpperCase()}`,
    "Effect Stream stdin/stdout round-trip",
  )
})

export const case_ = defineCase(
  "tool-middleware",
  "Effect tool call and standalone/combined middleware component worlds",
  run,
)
