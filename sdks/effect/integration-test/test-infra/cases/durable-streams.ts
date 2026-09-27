import { Effect } from "effect"
import { deepStrictEqual } from "node:assert/strict"
import { createServer, type ServerResponse } from "node:http"
import { setTimeout as delay } from "node:timers/promises"
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
        lastBody: Buffer
        tuple: string
        producer: string
        sequence: number
        closed: boolean
        attempts: number
        reads: number
      }
    >()
    let authenticated = 0
    const cancellationPaths: string[] = []
    let heldResponse: ServerResponse | undefined
    let heldTimer: ReturnType<typeof setTimeout> | undefined
    let released = false
    let notifyHeld!: () => void
    const heldStarted = new Promise<void>((resolve) => {
      notifyHeld = resolve
    })
    const peer = createServer((request, response) => {
      const handle = async () => {
        const url = new URL(request.url!, "http://peer")
        if (request.headers.authorization !== "Bearer effect-durable-streams-test-token") {
          failures.push("Missing or invalid borrowed bearer capability")
          response.writeHead(401).end()
          return
        }
        authenticated++
        if (url.pathname.startsWith("/cancel/")) {
          cancellationPaths.push(url.pathname)
          if (request.method !== "GET" || url.searchParams.get("offset") !== "-1")
            throw new Error("Cancellation control requires an initial read")
          const headers = {
            "content-type": "application/octet-stream",
            "stream-next-offset": "cancel-opaque:1",
            "stream-up-to-date": "true",
            "stream-closed": "true",
          }
          if (url.pathname === "/cancel/held") {
            if (heldResponse) throw new Error("Unexpected held-read retry")
            heldResponse = response
            heldTimer = setTimeout(() => {
              failures.push("Read cancellation did not release the held response within 10 seconds")
              response.writeHead(500).end()
            }, 10000)
            heldTimer.unref()
            notifyHeld()
            return
          }
          if (url.pathname === "/cancel/started") {
            await Promise.race([
              heldStarted,
              delay(10000, undefined, { ref: false }).then(() => {
                throw new Error("Held read never started")
              }),
            ])
            if (heldResponse!.headersSent) throw new Error("Held read settled before cancellation")
            response.writeHead(200, headers).end(Buffer.from([1]))
            return
          }
          if (url.pathname === "/cancel/release") {
            if (!heldResponse || heldResponse.headersSent || heldResponse.destroyed)
              throw new Error("Held response was not pending when cancellation completed")
            clearTimeout(heldTimer)
            heldResponse.writeHead(200, headers)
            await new Promise<void>((resolve) => {
              heldResponse!.end(Buffer.from([11]), resolve)
            })
            released = true
            response.writeHead(200, headers).end(Buffer.from([29]))
            return
          }
          if (url.pathname !== "/cancel/probe" || !released || !heldResponse?.writableFinished)
            throw new Error("Subsequent invocation ran before the native read settled")
          response.writeHead(200, headers).end(Buffer.from([41, 203]))
          return
        }
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
          const producer = JSON.stringify([
            request.headers["producer-id"],
            request.headers["producer-epoch"],
          ])
          const sequence = Number(request.headers["producer-seq"])
          const closed = request.headers["stream-closed"] === "true"
          if (
            request.headers["content-type"] !== contentType ||
            !Number.isSafeInteger(sequence) ||
            !request.headers["producer-id"]
          ) {
            failures.push("Append omitted media type or producer identity")
            response.writeHead(400).end()
            return
          }
          const previous = streams.get(url.pathname)
          const duplicate = previous?.sequence === sequence
          if (
            previous &&
            (duplicate
              ? previous.tuple !== tuple ||
                !previous.lastBody.equals(body) ||
                previous.closed !== closed
              : previous.producer !== producer ||
                previous.closed ||
                sequence !== previous.sequence + 1)
          ) {
            failures.push("Retry changed the committed producer tuple, body or close flag")
            response.writeHead(409).end()
            return
          }
          if (
            !duplicate &&
            (url.pathname === "/json"
              ? sequence !== 0 || !closed
              : sequence === 0
                ? closed
                : sequence !== 1 || !closed || body.length !== 0)
          ) {
            failures.push("Expected atomic JSON close or bytes append followed by close-only")
            response.writeHead(400).end()
            return
          }
          const state = previous ?? {
            body,
            lastBody: body,
            tuple,
            producer,
            sequence,
            closed,
            attempts: 0,
            reads: 0,
          }
          if (previous && !duplicate) {
            state.body = Buffer.concat([state.body, body])
            state.lastBody = body
            state.tuple = tuple
            state.sequence = sequence
            state.closed = closed
          }
          state.attempts++
          streams.set(url.pathname, state)
          // Commit before losing the response. Retry must deduplicate, not append again.
          if (url.pathname === "/json" && state.attempts === 1) {
            response.destroy()
            return
          }
          response.setHeader("producer-epoch", request.headers["producer-epoch"]!)
          response.setHeader("producer-seq", request.headers["producer-seq"]!)
          if (closed) response.setHeader("stream-closed", "true")
          if (!duplicate) response.setHeader("stream-next-offset", "receipt-opaque:1")
          response.writeHead(duplicate ? 204 : 200).end()
          return
        }
        const state = streams.get(url.pathname)
        const checkpoint =
          state?.reads === 1 && url.pathname === "/bytes" ? "middle-opaque:4" : "-1"
        if (
          request.method !== "GET" ||
          !state ||
          !state.closed ||
          url.searchParams.get("offset") !== checkpoint
        ) {
          failures.push("Read did not use the original opaque checkpoint after append")
          response.writeHead(400).end()
          return
        }
        const firstBytes = url.pathname === "/bytes" && state.reads === 0
        state.reads++
        response
          .writeHead(200, {
            "content-type": contentType,
            "stream-next-offset": firstBytes ? "middle-opaque:4" : "tail-opaque:9",
            ...(!firstBytes ? { "stream-up-to-date": "true", "stream-closed": "true" } : {}),
          })
          .end(
            url.pathname === "/bytes"
              ? firstBytes
                ? state.body.subarray(0, 1)
                : state.body.subarray(1)
              : state.body,
          )
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
              clearTimeout(heldTimer)
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
      ["cancelRead", "cancel", list([{ kind: "u8", value: 29 }])],
      ["readAfterCancel", "cancel", list([41, 203].map((value) => ({ kind: "u8", value })))],
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
    yield* expectEqual(
      streams.get("/bytes")!.attempts,
      2,
      "bytes append and close-only reuse producer",
    )
    yield* expectEqual(streams.get("/json")!.reads, 1, "one final JSON batch")
    yield* expectEqual(streams.get("/bytes")!.reads, 2, "two bytes batches with opaque checkpoint")
    yield* expectEqual(
      JSON.stringify(cancellationPaths.slice().sort()),
      JSON.stringify(["/cancel/held", "/cancel/probe", "/cancel/release", "/cancel/started"]),
      "one held read, bounded cancellation handshake, then subsequent invocation",
    )
    yield* expectEqual(authenticated, 11, "all attempts use the captured shared secret")
  }),
)

export const case_ = defineCase(
  "durable-streams",
  "External JSON/bytes, uncertain append deduplication, shared secret borrow, native stream RPC",
  run,
)
