/**
 * WebSocketAgent case — exercises the SDK's `Websocket.connect`
 * outbound binding by driving the deployed agent against a public
 * echo server (`wss://ws.postman-echo.com/raw`).
 *
 *   - `echo` sends one frame and expects the same string back.
 *   - `echoMany` sends an array of frames and expects an in-order
 *     array of replies.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `ws-${session.stamp}`
  const r = `WebSocketAgent("${name}")`

  const single = `hello-ws-${session.stamp}`
  const echo = yield* liftCliError(cli.invoke(r, "echo", [`"${single}"`]))
  yield* expectMatch(echo.stdout, /tag[^a-zA-Z]+ok/, "echo returns tag = ok")
  yield* expectMatch(
    echo.stdout,
    new RegExp(single),
    "echo reply round-trips the request through Websocket.connect",
  )

  const messages = ["alpha", "beta", "gamma"]
  const messagesArg = `[${messages.map((m) => `"${m}"`).join(",")}]`
  const many = yield* liftCliError(cli.invoke(r, "echoMany", [messagesArg]))
  yield* expectMatch(many.stdout, /tag[^a-zA-Z]+ok/, "echoMany returns tag = ok")
  for (const m of messages) {
    yield* expectMatch(many.stdout, new RegExp(m), `echoMany replies contain ${m}`)
  }

  // Oplog should record the host's websocket-connection.connect /
  // send / receive trail.
  const oplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(
    oplog.stdout,
    /websocket/i,
    "WebSocketAgent oplog records golem:websocket host calls",
  )
})

export const case_ = defineCase(
  "websocket",
  "WebSocketAgent: outbound Websocket.connect echo round-trips (single + multi-frame)",
  run,
)
