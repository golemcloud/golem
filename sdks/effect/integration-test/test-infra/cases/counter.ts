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
import * as NodeHttp from "node:http"
import { GolemCli } from "../harness/golem-cli.ts"
import {
  TestFailure,
  TestSession,
  defineCase,
  expectEqual,
  expectMatch,
  liftCliError,
  updateTolerant,
} from "../harness/case.ts"

const httpHost = "effect-golem.localhost:9006"

const httpGet = (
  pathname: string,
  headers?: Record<string, string>,
): Effect.Effect<{ status: number; body: string; headers: Headers }, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    const res = yield* Effect.tryPromise({
      try: () => fetch(`http://${httpHost}${pathname}`, { headers }),
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
    return { status: res.status, body, headers: res.headers }
  })

const httpGetBytes = (
  pathname: string,
): Effect.Effect<
  { status: number; body: Uint8Array; headers: Headers },
  TestFailure,
  TestSession
> =>
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
      try: async () => new Uint8Array(await res.arrayBuffer()),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP GET ${pathname} body read failed`,
          diagnostic: String(cause),
        }),
    })
    return { status: res.status, body, headers: res.headers }
  })

const httpPost = (
  pathname: string,
  body?: unknown,
): Effect.Effect<{ status: number; body: string; headers: Headers }, TestFailure, TestSession> =>
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
    return { status: res.status, body: text, headers: res.headers }
  })

const httpGetWithRepeatedHeader = (
  pathname: string,
  headerName: string,
  values: ReadonlyArray<string>,
): Effect.Effect<{ status: number; body: string }, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    return yield* Effect.tryPromise({
      try: () =>
        new Promise<{ status: number; body: string }>((resolve, reject) => {
          const req = NodeHttp.get(
            {
              hostname: "127.0.0.1",
              port: 9006,
              path: pathname,
              headers: {
                host: httpHost,
                [headerName]: [...values],
              },
            },
            (res) => {
              res.setEncoding("utf8")
              let body = ""
              res.on("data", (chunk: string) => {
                body += chunk
              })
              res.on("end", () => resolve({ status: res.statusCode ?? 0, body }))
            },
          )
          req.on("error", reject)
        }),
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `HTTP GET ${pathname} with repeated ${headerName} headers threw`,
          diagnostic: String(cause),
        }),
    })
  })

const expectStructuredBadRequest = (
  response: { status: number; body: string; headers: Headers },
  code: string,
  error: RegExp,
  description: string,
): Effect.Effect<void, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    yield* expectEqual(response.status, 400, `${description} status`)
    yield* expectMatch(
      response.headers.get("content-type") ?? "",
      /^application\/json(?:;|$)/,
      `${description} content type`,
    )
    const body = yield* Effect.try({
      try: () => JSON.parse(response.body) as { code?: unknown; errors?: unknown },
      catch: (cause) =>
        new TestFailure({
          testName: session.currentTest,
          message: `${description} body is not valid JSON`,
          diagnostic: String(cause),
        }),
    })
    yield* expectEqual(body.code, code, `${description} error code`)
    yield* expectEqual(Array.isArray(body.errors), true, `${description} errors is an array`)
    const errors = Array.isArray(body.errors) ? body.errors : []
    yield* expectEqual(errors.length, 1, `${description} error count`)
    yield* expectMatch(String(errors[0]), error, `${description} error message`)
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

  yield* expectStructuredBadRequest(
    yield* httpGet(`/counters/${counterName}/add`),
    "REQUEST_MISSING_VALUE",
    /Expected single value values to be provided, but found none/,
    "missing required query value",
  )
  yield* expectStructuredBadRequest(
    yield* httpGet(`/counters/${counterName}/add?by=not-a-number`),
    "REQUEST_VALUE_PARSING_FAILED",
    /Failed parsing value; Provided: not-a-number; Expected type: f64/,
    "malformed scalar query value",
  )
  yield* expectStructuredBadRequest(
    yield* httpGet(`/counters/${counterName}/add?by=1&by=2`),
    "REQUEST_TOO_MANY_VALUES",
    /Expected single value values to be provided, but found too many/,
    "repeated scalar query value",
  )

  const textResponse = yield* httpGet(`/counters/${counterName}/response/text`)
  yield* expectEqual(textResponse.status, 200, "plain-text response status")
  yield* expectEqual(textResponse.body, "hello from Effect", "plain-text response body")
  yield* expectEqual(
    textResponse.headers.get("content-type"),
    "text/plain; charset=utf-8",
    "plain-text response content type",
  )
  yield* expectEqual(
    textResponse.headers.get("content-language"),
    "en",
    "plain-text response language",
  )

  const binaryResponse = yield* httpGetBytes(`/counters/${counterName}/response/binary`)
  yield* expectEqual(binaryResponse.status, 200, "binary response status")
  yield* expectEqual(binaryResponse.body.join(","), "0,127,255", "binary response body")
  yield* expectEqual(
    binaryResponse.headers.get("content-type"),
    "application/octet-stream",
    "binary response content type",
  )

  const jsonResponse = yield* httpGet(`/counters/${counterName}/response/json`)
  yield* expectEqual(jsonResponse.status, 200, "JSON response status")
  const jsonBody = yield* Effect.try({
    try: () => JSON.parse(jsonResponse.body) as { kind?: unknown; count?: unknown },
    catch: (cause) =>
      new TestFailure({
        testName: session.currentTest,
        message: "JSON response body is not valid JSON",
        diagnostic: String(cause),
      }),
  })
  yield* expectEqual(jsonBody.kind, "counter", "JSON response kind")
  yield* expectEqual(jsonBody.count, 16, "JSON response count")
  yield* expectEqual(
    jsonResponse.headers.get("content-type"),
    "application/json",
    "JSON response content type",
  )

  const emptyResponse = yield* httpGet(`/counters/${counterName}/response/empty`)
  yield* expectEqual(emptyResponse.status, 204, "empty response status")
  yield* expectEqual(emptyResponse.body, "", "empty response body")

  const collectionQuery = new URLSearchParams([
    ["tag", "alpha"],
    ["tag", "beta"],
    ["tag", "gamma"],
  ])
  const collectionResponse = yield* httpGetWithRepeatedHeader(
    `/counters/${counterName}/bindings/collections?${collectionQuery}`,
    "X-Score",
    ["1", "2.5", "-3"],
  )
  yield* expectEqual(collectionResponse.status, 200, "collection bindings response status")
  const collectionBody = yield* Effect.try({
    try: () => JSON.parse(collectionResponse.body) as { tags?: unknown; scores?: unknown },
    catch: (cause) =>
      new TestFailure({
        testName: session.currentTest,
        message: "collection bindings response is not valid JSON",
        diagnostic: String(cause),
      }),
  })
  yield* expectEqual(
    Array.isArray(collectionBody.tags) ? collectionBody.tags.join(",") : "not-an-array",
    "alpha,beta,gamma",
    "repeated query parameters bind to a string array",
  )
  yield* expectEqual(
    Array.isArray(collectionBody.scores) ? collectionBody.scores.join(",") : "not-an-array",
    "1,2.5,-3",
    "repeated header instances bind to a number array",
  )
  yield* expectStructuredBadRequest(
    yield* httpGet(`/counters/${counterName}/bindings/collections?tag=alpha`, {
      "X-Score": "not-a-number",
    }),
    "REQUEST_VALUE_PARSING_FAILED",
    /Failed parsing value; Provided: not-a-number; Expected type: f64/,
    "unparsable collection header value",
  )

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
