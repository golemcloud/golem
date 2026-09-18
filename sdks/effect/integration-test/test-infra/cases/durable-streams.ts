import { Effect } from "effect"
import { deepStrictEqual } from "node:assert/strict"
import { createServer } from "node:http"
import { defineCase, expectEqual, liftCliError, TestFailure, TestSession } from "../harness/case.ts"
import { GolemCli } from "../harness/golem-cli.ts"

const run = Effect.scoped(
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const failures: string[] = []
    const streams = new Map<
      string,
      {
        body: Buffer
        tuple: string
        closed: boolean
        attempts: number
        reads: number
      }
    >()
    let authenticated = 0
    const peer = createServer((request, response) => {
      const handle = async () => {
        const url = new URL(request.url!, "http://peer")
        if (request.headers.authorization !== "Bearer effect-durable-streams-test-token") {
          failures.push("Missing or invalid borrowed bearer capability")
          response.writeHead(401).end()
          return
        }
        authenticated++
        const contentType =
          url.pathname === "/json" ? "application/json" : "application/octet-stream"
        if (request.method === "POST") {
          const chunks: Buffer[] = []
          for await (const chunk of request) chunks.push(Buffer.from(chunk))
          const body = Buffer.concat(chunks)
          const tuple = JSON.stringify([
            request.headers["producer-id"],
            request.headers["producer-epoch"],
            request.headers["producer-seq"],
          ])
          const closed = request.headers["stream-closed"] === "true"
          if (
            request.headers["content-type"] !== contentType ||
            !closed ||
            !request.headers["producer-id"]
          ) {
            failures.push("Append omitted media type, producer identity or atomic close")
            response.writeHead(400).end()
            return
          }
          const previous = streams.get(url.pathname)
          if (
            previous &&
            (previous.tuple !== tuple || !previous.body.equals(body) || previous.closed !== closed)
          ) {
            failures.push("Retry changed the committed producer tuple, body or close flag")
            response.writeHead(409).end()
            return
          }
          const state = previous ?? { body, tuple, closed, attempts: 0, reads: 0 }
          state.attempts++
          streams.set(url.pathname, state)
          // Commit before losing the response. Retry must deduplicate, not append again.
          if (url.pathname === "/json" && state.attempts === 1) {
            response.destroy()
            return
          }
          response.setHeader("producer-epoch", request.headers["producer-epoch"]!)
          response.setHeader("producer-seq", request.headers["producer-seq"]!)
          response.setHeader("stream-closed", "true")
          if (!previous) response.setHeader("stream-next-offset", "receipt-opaque:1")
          response.writeHead(previous ? 204 : 200).end()
          return
        }
        const state = streams.get(url.pathname)
        if (request.method !== "GET" || !state || url.searchParams.get("offset") !== "-1") {
          failures.push("Read did not use the original opaque checkpoint after append")
          response.writeHead(400).end()
          return
        }
        state.reads++
        response
          .writeHead(200, {
            "content-type": contentType,
            "stream-next-offset": "tail-opaque:9",
            "stream-up-to-date": "true",
            "stream-closed": "true",
          })
          .end(state.body)
      }
      void handle().catch(() => {
        failures.push("HTTP peer handler failed")
        response.writeHead(500).end()
      })
    })
    const port = yield* Effect.acquireRelease(
      Effect.tryPromise({
        try: () =>
          new Promise<number>((resolve, reject) => {
            peer.once("error", reject)
            peer.listen(0, "127.0.0.1", () => {
              peer.removeListener("error", reject)
              const address = peer.address()
              if (address === null || typeof address === "string") reject(new Error("No peer port"))
              else resolve(address.port)
            })
          }),
        catch: () =>
          new TestFailure({ testName: session.currentTest, message: "Could not start HTTP peer" }),
      }),
      () =>
        Effect.promise(
          () =>
            new Promise<void>((resolve) => {
              peer.closeAllConnections()
              peer.close(() => resolve())
            }),
        ),
    )
    const ref = `EffectDurableStreams("ds-${session.stamp}")`
    const list = (elements: unknown[]) => ({ kind: "list", value: { elements } })
    for (const [method, path, expected] of [
      [
        "jsonRoundtrip",
        "json",
        list([
          list(["first", "a,b"].map((value) => ({ kind: "string", value }))),
          list([{ kind: "string", value: "last" }]),
        ]),
      ],
      ["forwardBytes", "bytes", list([3, 249, 17].map((value) => ({ kind: "u8", value })))],
    ] as const) {
      const output = yield* liftCliError(
        cli.run([
          "--local",
          "--format",
          "json",
          "agent",
          "invoke",
          "--no-stream",
          ref,
          method,
          JSON.stringify(`http://127.0.0.1:${port}/${path}`),
        ]),
      )
      yield* Effect.try({
        try: () => {
          const result = JSON.parse(output.stdout)
          deepStrictEqual(result.$type, "agent.invoke")
          deepStrictEqual(result.resultJson.value, expected)
        },
        catch: () =>
          new TestFailure({
            testName: session.currentTest,
            message: `${method} returned unexpected structured output`,
            diagnostic: output.stdout,
          }),
      })
    }
    yield* expectEqual(JSON.stringify(failures), "[]", "peer protocol assertions")
    yield* expectEqual(
      streams.get("/json")!.body.toString(),
      '[["first","a,b"],["last"]]',
      "JSON outer batch and nested arrays",
    )
    yield* expectEqual(
      streams.get("/json")!.attempts,
      2,
      "lost acknowledgement retries identical append",
    )
    yield* expectEqual(
      streams.get("/bytes")!.body.toString("hex"),
      "03f911",
      "exact committed bytes",
    )
    yield* expectEqual(streams.get("/bytes")!.attempts, 1, "one bytes append")
    yield* expectEqual(streams.get("/json")!.reads, 1, "one final JSON batch")
    yield* expectEqual(streams.get("/bytes")!.reads, 1, "one final bytes batch")
    yield* expectEqual(authenticated, 5, "all attempts borrow the shared secret")
  }),
)

export const case_ = defineCase(
  "durable-streams",
  "External JSON/bytes, uncertain append deduplication, shared secret borrow, native stream RPC",
  run,
)
