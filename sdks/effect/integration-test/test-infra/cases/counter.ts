/**
 * Counter case — exercises:
 *
 *   - basic invoke surface (value / increment / add / reset)
 *   - config-driven methods (currentGreeting reads `greeting` from
 *     `golem.yaml`; keyTail reads the secret `apiKey` from
 *     `secretDefaults`)
 *   - HTTP routes via the deployed httpApi (effect-golem.localhost:9006)
 *   - snapshot drill (everyN(10)) + update --await
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import {
  TestFailure,
  TestSession,
  defineCase,
  expectMatch,
  liftCliError,
  updateTolerant,
} from "../harness/case.ts"

const httpHost = "effect-golem.localhost:9006"

const httpGet = (
  pathname: string,
): Effect.Effect<{ status: number; body: string }, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    const res = yield* Effect.tryPromise({
      try: () => fetch(`http://${httpHost}${pathname}`),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP GET ${pathname} threw`,
          diagnostic: String(cause),
        }),
    })
    const body = yield* Effect.tryPromise({
      try: () => res.text(),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP GET ${pathname} body read failed`,
          diagnostic: String(cause),
        }),
    })
    return { status: res.status, body }
  })

const httpPost = (
  pathname: string,
  body?: unknown,
): Effect.Effect<{ status: number; body: string }, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    const res = yield* Effect.tryPromise({
      try: () =>
        fetch(`http://${httpHost}${pathname}`, {
          method: "POST",
          headers: body !== undefined ? { "content-type": "application/json" } : {},
          body: body !== undefined ? JSON.stringify(body) : undefined,
        }),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP POST ${pathname} threw`,
          diagnostic: String(cause),
        }),
    })
    const text = yield* Effect.tryPromise({
      try: () => res.text(),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP POST ${pathname} body read failed`,
          diagnostic: String(cause),
        }),
    })
    return { status: res.status, body: text }
  })

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const counterName = `ctr-${session.stamp}`
  const r = `Counter("${counterName}")`

  // ---- CLI invocation surface ----
  const v0 = yield* liftCliError(cli.invoke(r, "value"))
  yield* expectMatch(v0.stdout, /\b0\b/, "Counter.value initial = 0")

  yield* liftCliError(cli.invoke(r, "increment"))
  yield* liftCliError(cli.invoke(r, "add", ["4"]))
  const v1 = yield* liftCliError(cli.invoke(r, "value"))
  yield* expectMatch(v1.stdout, /\b5\b/, "Counter.value after increment+add(4) = 5")

  // Config-driven methods.
  const greeting = yield* liftCliError(cli.invoke(r, "currentGreeting"))
  yield* expectMatch(
    greeting.stdout,
    /Hello from golem\.yaml/,
    "Counter.currentGreeting reads greeting from golem.yaml config",
  )

  const keyTail = yield* liftCliError(cli.invoke(r, "keyTail"))
  // golem.yaml secretDefault.apiKey ends with "1234"
  yield* expectMatch(keyTail.stdout, /1234/, "Counter.keyTail reads tail of apiKey secret")

  // owner / caller principals.
  yield* liftCliError(cli.invoke(r, "owner"))
  yield* liftCliError(cli.invoke(r, "caller"))

  // ---- HTTP routes ----
  const httpVal = yield* httpGet(`/counters/${counterName}/value`)
  yield* expectMatch(
    httpVal.body,
    /\b5\b/,
    `HTTP GET /counters/${counterName}/value returns current count`,
  )

  yield* httpPost(`/counters/${counterName}/increment`)
  const httpVal2 = yield* httpGet(`/counters/${counterName}/value`)
  yield* expectMatch(
    httpVal2.body,
    /\b6\b/,
    "HTTP POST /increment increments via the host's HTTP API",
  )

  // GET /add?by={by} (query-bound parameter).
  yield* httpGet(`/counters/${counterName}/add?by=10`)
  const httpVal3 = yield* httpGet(`/counters/${counterName}/value`)
  yield* expectMatch(httpVal3.body, /\b16\b/, "HTTP GET /add?by=10 adds via query parameter")

  // ---- Snapshot drill + update ----
  for (let i = 0; i < 10; i++) {
    yield* liftCliError(cli.invoke(r, "add", ["1"]))
  }
  const oplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(oplog.stdout, /SNAPSHOT/i, "Counter oplog has SNAPSHOT entry")

  yield* updateTolerant(r, "manual")
  const valAfterUpdate = yield* liftCliError(cli.invoke(r, "value"))
  yield* expectMatch(
    valAfterUpdate.stdout,
    /\b26\b/,
    "Counter.value after update preserves state (16 + 10 × add(1) = 26)",
  )
})

export const case_ = defineCase(
  "counter",
  "Counter: invoke + HTTP routes + config + snapshot + update drill",
  run,
)
