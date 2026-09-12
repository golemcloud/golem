import { Effect } from "effect"
import { defineCase, expectMatch, liftCliError, TestFailure, TestSession } from "../harness/case.ts"
import { GolemCli } from "../harness/golem-cli.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const ref = `EffectP3Caller("p3-${session.stamp}")`

  const streamed = yield* liftCliError(cli.invoke(ref, "streamRoundtrip"))
  yield* expectMatch(streamed.stdout, /2[^0-9]+4[^0-9]+6/, "Effect Stream RPC round-trip")

  const nested = yield* liftCliError(cli.invoke(ref, "nestedStreamCancellation"))
  yield* expectMatch(nested.stdout, /rpc:alpha:2\+3/, "nested RPC stream transforms struct item")
  yield* expectMatch(nested.stdout, /rpc:beta:5\+8\+13/, "nested RPC stream returns second item")
  // Closing the receiving endpoint does not synchronously acknowledge upstream producer cleanup.
  yield* expectMatch(
    nested.stdout,
    /inputStoppedEarly[^a-z]+true/i,
    "early cancellation stops the source before exhaustion while permitting prefetch",
  )

  const ephemeral = yield* liftCliError(cli.invoke(ref, "ephemeralRoundtrip"))
  yield* expectMatch(ephemeral.stdout, /p3-.*:one/, "first ephemeral invocation returns value")
  yield* expectMatch(ephemeral.stdout, /p3-.*:two/, "second ephemeral invocation returns value")
  yield* expectMatch(
    ephemeral.stdout,
    /identitiesDiffer[^a-z]+true/i,
    "ephemeral metadata has unique agent identity",
  )
  yield* expectMatch(
    ephemeral.stdout,
    /idempotencyKeysPresent[^a-z]+true/i,
    "ephemeral metadata returns idempotency keys",
  )
  yield* expectMatch(
    ephemeral.stdout,
    /firstAgentId[^\n]+[0-9a-f]{8}-/i,
    "ephemeral metadata exposes generated agent ID",
  )

  const cancelled = yield* liftCliError(cli.invoke(ref, "cancelledSchedule"))
  yield* expectMatch(cancelled.stdout, /\b0\b/, "cancelled scheduled RPC does not execute")
})

export const case_ = defineCase(
  "p3",
  "Preview 3 nested stream RPC/cancellation, ephemeral identity, and scheduled cancellation",
  run,
)
