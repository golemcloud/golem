/**
 * WebSocketAgent — exercises the `Websocket` namespace end-to-end
 * against a real Golem runtime, using a public WebSocket echo server.
 *
 * The host's `golem:websocket/client@1.5.0` binding is wrapped in an
 * Effect-idiomatic `Socket` (`effect/unstable/socket`) by the SDK, so
 * this agent can use the same `runString` / writer / `Effect.scoped`
 * patterns a regular Effect application would use against a browser
 * or Node WebSocket.
 *
 * The result types are intentionally union structs (rather than typed
 * Effect failures) so the dispatcher can encode them through the
 * method's `success` schema — this avoids the host's
 * `AgentError.custom-error` encoding path and makes the round-trip
 * runtime-portable.
 *
 * Drill (run from `integration-test`):
 *
 * ```
 * golem -L agent invoke -n 'WebSocketAgent("demo")' echo '"hello-from-golem"'
 * # => { tag: "ok", reply: "hello-from-golem" }
 * golem -L agent invoke -n 'WebSocketAgent("demo")' echo-many '["alpha","beta","gamma"]'
 * # => { tag: "ok", replies: ["alpha","beta","gamma"] }
 * ```
 *
 * The oplog (`golem -L agent oplog 'WebSocketAgent("demo")'`) shows
 * the durable host calls
 * `golem:websocket/client::websocket-connection.connect`,
 * `golem:websocket/client::websocket-connection.send`,
 * `io::poll::pollable::ready` (one per inbound frame), and
 * `golem:websocket/client::websocket-connection.receive` —
 * confirming the SDK's `subscribe()` -> `pollable.promise()` ->
 * `receive()` round-trip is mapped 1:1 onto the host bindings.
 */
import { Effect, Fiber, Queue, Schema } from "effect"
import { defineAgent, method, Websocket } from "effect-golem"

// `wss://ws.postman-echo.com/raw` is Postman's free WebSocket echo
// service. It echoes every text or binary frame back verbatim with no
// extra banner traffic, so we don't have to discard anything before
// the first user-visible frame arrives.
const ECHO_URL = "wss://ws.postman-echo.com/raw"

// Single struct with optional fields, mirroring the booking-saga
// integration agent's "tag-based result" idiom. Avoids the Schema.Union
// codec's "ambiguous union: multiple object members without distinct
// `_tag` discriminators" rejection (the SDK's discriminator key is
// `_tag`, but our outward shape uses `tag` so it round-trips cleanly
// through the host's Wasm Component Model encoding).
const EchoResult = Schema.Struct({
  tag: Schema.Literals(["ok", "error"]),
  reply: Schema.optional(Schema.String),
  code: Schema.optional(Schema.Literals(["timeout", "remote-error", "no-reply"])),
  message: Schema.optional(Schema.String),
})
type EchoResult = typeof EchoResult.Type

const EchoManyResult = Schema.Struct({
  tag: Schema.Literals(["ok", "error"]),
  replies: Schema.optional(Schema.Array(Schema.String)),
  code: Schema.optional(Schema.Literals(["timeout", "remote-error", "no-reply"])),
  message: Schema.optional(Schema.String),
})
type EchoManyResult = typeof EchoManyResult.Type

export const WebSocketAgent = defineAgent({
  name: "WebSocketAgent",
  description:
    "Exercises Websocket.connect + Socket.runString round-trips against a public echo server.",
  mode: "durable",
  constructorParams: { name: Schema.String },
  methods: {
    /** Send a single text frame and return the first echo reply. */
    echo: method({
      params: { message: Schema.String },
      success: EchoResult,
      description:
        "Send one text frame to the echo server and return the reply (or a tagged error).",
    }),
    /** Send several frames, return all replies in order. */
    echoMany: method({
      params: { messages: Schema.Array(Schema.String) },
      success: EchoManyResult,
      description:
        "Send N text frames to the echo server and return the N replies (or a tagged error).",
    }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    yield* Effect.logInfo("WebSocketAgent constructed").pipe(Effect.annotateLogs({ name }))

    // Single helper that opens a fresh connection, sends + receives,
    // and tears it down. Each invocation uses its own scope so the
    // connection is deterministically closed when the method returns.
    const echoOnce = (messages: ReadonlyArray<string>) =>
      Effect.scoped(
        Effect.gen(function* () {
          const expected = messages.length

          // Open the WebSocket. Connect failures are caught and
          // returned as a tagged result instead of bubbled up as a
          // typed failure.
          const sockOrErr = yield* Effect.result(
            Websocket.connect(ECHO_URL, {
              closeCodeIsError: (c) => c !== 1000,
            }),
          )
          if (sockOrErr._tag === "Failure") {
            return {
              tag: "error" as const,
              code: "remote-error" as const,
              message: `connect failed: ${sockOrErr.failure.message}`,
            }
          }
          const sock = sockOrErr.success

          // Materialize inbound text frames into a Queue so the
          // top-level method body can take exactly N replies with a
          // timeout — `Socket.runString` is a push-based blocking
          // loop, so we adapt it to a pull-based interface here.
          const inbound = yield* Queue.unbounded<string>()

          const reader = yield* Effect.forkChild(
            sock.runString((line) => {
              Queue.offerUnsafe(inbound, line)
            }),
          )

          // Send all messages.
          const sendOrErr = yield* Effect.result(
            Effect.scoped(
              Effect.gen(function* () {
                const write = yield* sock.writer
                for (const msg of messages) {
                  yield* write(msg)
                }
              }),
            ),
          )
          if (sendOrErr._tag === "Failure") {
            yield* Fiber.interrupt(reader)
            return {
              tag: "error" as const,
              code: "remote-error" as const,
              message: `send failed: ${sendOrErr.failure.message}`,
            }
          }

          // Take exactly `expected` replies, with a per-take timeout.
          const replies: string[] = []
          for (let i = 0; i < expected; i++) {
            const take = yield* Queue.take(inbound).pipe(Effect.timeoutOption("10 seconds"))
            if (take._tag === "None") {
              yield* Fiber.interrupt(reader)
              return {
                tag: "error" as const,
                code: "timeout" as const,
                message: `timeout waiting for reply #${i + 1} of ${expected}`,
              }
            }
            replies.push(take.value)
          }

          // Stop the read loop. The outer scope's finalizer will
          // issue close(1000, undefined) on the connection itself.
          yield* Fiber.interrupt(reader)

          return { tag: "ok" as const, replies }
        }),
      )

    return {
      echo: ({ message }) =>
        Effect.gen(function* () {
          const result = yield* echoOnce([message])
          if (result.tag === "error") {
            return {
              tag: "error" as const,
              code: result.code,
              message: result.message,
            } satisfies EchoResult
          }
          const first = (result.replies ?? [])[0]
          if (first === undefined) {
            return {
              tag: "error" as const,
              code: "no-reply" as const,
              message: "echo server returned no reply",
            } satisfies EchoResult
          }
          return { tag: "ok" as const, reply: first } satisfies EchoResult
        }),
      echoMany: ({ messages }) => echoOnce(messages),
    }
  }),
)
