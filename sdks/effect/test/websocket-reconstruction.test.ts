import { describe, expect, it } from "@effect/vitest"
import { Deferred, Effect, Fiber, Schema, Stream } from "effect"
import { Socket } from "effect/socket"
import * as Websocket from "../src/Websocket.js"
import * as WsFake from "./host/WsFake.js"
import * as WsMock from "./mocks/golem-websocket-client.js"

describe("websocket reconstruction", () => {
  it("recognizes only the structural session-loss shape", () => {
    const structural: unknown = {
      _tag: "SocketError",
      reason: { _tag: "SocketReadError", cause: { _tag: "SessionLost" } },
    }
    expect(Websocket.isSessionLost(structural)).toBe(true)
    if (Websocket.isSessionLost(structural)) {
      expect(structural.reason.cause._tag).toBe("SessionLost")
      // @ts-expect-error Structural recognition does not promise an Error instance.
      expect(structural.message).toBeUndefined()
    }
    for (const value of [
      null,
      "SessionLost",
      { _tag: "SocketError" },
      { _tag: "SocketError", reason: { _tag: "SocketCloseError", cause: { _tag: "SessionLost" } } },
      { _tag: "SocketError", reason: { _tag: "SocketReadError", cause: null } },
      { _tag: "SocketError", reason: { _tag: "SocketWriteError", cause: { _tag: "Other" } } },
    ])
      expect(Websocket.isSessionLost(value)).toBe(false)
  })

  it.effect("preserves unrelated explicit close failures and keeps finalization best effort", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test", undefined)
      conn.close = () => {
        throw { tag: "other", val: "close rejected" }
      }
      yield* fake.setResponder(() => conn)
      yield* Effect.scoped(
        Effect.gen(function* () {
          const socket = yield* Websocket.connect("wss://test", { closeCodeIsError: () => false })
          const reader = yield* Effect.forkChild(socket.runString<void, never, never>(() => {}))
          const writer = yield* socket.writer
          yield* writer.write(new Socket.CloseEvent(1000))
          yield* Fiber.join(reader)
        }),
      ).pipe(Effect.provide(fake.layer))
    }),
  )
  for (const constructor of ["layer", "channel"] as const) {
    it.effect(`forwards the policy through ${constructor}`, () =>
      Effect.gen(function* () {
        const fake = yield* WsFake.make
        const conn = new WsMock.WebsocketConnection("wss://test", undefined)
        conn.receive = () => Promise.reject({ tag: "session-lost" })
        yield* fake.setResponder(() => conn)
        const options = { reconstructionPolicy: "report-connection-loss" } as const
        const run =
          constructor === "layer"
            ? Effect.gen(function* () {
                const socket = yield* Socket.Socket
                const pull = yield* Socket.readerString(socket)
                yield* pull
              }).pipe(Effect.provide(Websocket.layer("wss://test", options)))
            : Stream.runDrain(
                Stream.empty.pipe(
                  Stream.pipeThroughChannel(Websocket.makeChannel("wss://test", options)),
                ),
              )
        yield* Effect.exit(run.pipe(Effect.provide(fake.layer)))
        expect((yield* fake.recordedConnects)[0]?.reconstructionPolicy).toBe(
          "report-connection-loss",
        )
      }),
    )
  }
  it.effect("cancels initialization on host receive loss before the next generation connects", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const old = new WsMock.WebsocketConnection("wss://old", undefined)
      const fresh = new WsMock.WebsocketConnection("wss://new", undefined)
      const events: string[] = []
      let rejectReceive: ((error: unknown) => void) | undefined
      old.receive = () =>
        new Promise((_, reject) => {
          rejectReceive = reject
        })
      yield* fake.setResponder((url) => {
        events.push(url)
        return url === "wss://old" ? old : fresh
      })
      const failed = yield* Effect.flip(
        Effect.scoped(
          Effect.gen(function* () {
            const socket = yield* Websocket.connect("wss://old", {
              reconstructionPolicy: "report-connection-loss",
            })
            const reader = yield* Effect.forkChild(socket.runString<void, never, never>(() => {}))
            const initialize = Effect.gen(function* () {
              const writer = yield* socket.writer
              yield* writer.write("initialize")
              yield* Effect.sync(() => rejectReceive!({ tag: "session-lost" }))
              yield* Effect.never
              yield* writer.write("must-not-forward")
            }).pipe(Effect.ensuring(Effect.sync(() => events.push("old-initialization-joined"))))
            yield* Effect.raceFirst(Fiber.join(reader), initialize)
          }),
        ),
      ).pipe(Effect.provide(fake.layer))
      expect(Websocket.isSessionLost(failed)).toBe(true)
      expect(WsMock.__outboundCloses(old)).toHaveLength(1)
      yield* Effect.scoped(
        Websocket.connect("wss://new", { reconstructionPolicy: "report-connection-loss" }),
      ).pipe(Effect.provide(fake.layer))
      expect(events).toEqual(["wss://old", "old-initialization-joined", "wss://new"])
      expect(WsMock.__outbound(old)).toEqual([{ tag: "text", val: "initialize" }])
      expect(WsMock.__outbound(fresh)).toEqual([])
    }),
  )
  it.effect(
    "joins a lost generation and its pending latch writer before initializing a new socket",
    () =>
      Effect.gen(function* () {
        const events: string[] = []
        const lost = new Socket.SocketError({
          reason: new Socket.SocketReadError({ cause: { _tag: "SessionLost" } }),
        })
        const initializing = yield* Deferred.make<void>()
        const oldExit = yield* Effect.flip(
          Effect.scoped(
            Effect.gen(function* () {
              const old = yield* Websocket.fromConnection(
                Deferred.await(initializing).pipe(Effect.andThen(Effect.fail(lost))),
              )
              const writer = yield* old.writer
              const initialization = Effect.gen(function* () {
                yield* Deferred.succeed(initializing, undefined)
                yield* writer.write("old-initialize")
                events.push("old-forwarded")
              }).pipe(Effect.ensuring(Effect.sync(() => events.push("old-writer-joined"))))
              yield* Effect.raceFirst(
                old.runString<void, never, never>(() => {}),
                initialization,
              )
            }),
          ),
        )
        expect(oldExit).toBe(lost)
        expect(events).toEqual(["old-writer-joined"])
        const fake = yield* WsFake.make
        const conn = new WsMock.WebsocketConnection("wss://new", undefined)
        const reading = yield* Deferred.make<void>()
        conn.receive = () => {
          events.push("new-reader")
          Effect.runSync(Deferred.succeed(reading, undefined))
          return new Promise(() => {})
        }
        conn.send = (message) => {
          events.push(`new-${message.val}`)
        }
        yield* fake.setResponder(() => conn)
        yield* Effect.scoped(
          Effect.gen(function* () {
            const socket = yield* Websocket.connect("wss://new", {
              reconstructionPolicy: "report-connection-loss",
            })
            const reader = yield* Effect.forkChild(socket.runString<void, never, never>(() => {}))
            const initializeAndWrite = Effect.gen(function* () {
              yield* Deferred.await(reading)
              const writer = yield* socket.writer
              yield* writer.write("initialize")
              yield* writer.write("traffic")
            })
            yield* Effect.raceFirst(Fiber.join(reader), initializeAndWrite)
          }),
        ).pipe(Effect.provide(fake.layer))
        expect(events).toEqual(["old-writer-joined", "new-reader", "new-initialize", "new-traffic"])
        expect((yield* fake.recordedConnects)[0]?.reconstructionPolicy).toBe(
          "report-connection-loss",
        )
      }),
  )
  for (const operation of ["send", "receive", "close"] as const) {
    it.effect(`preserves ${operation} loss through the SocketError schema`, () =>
      Effect.gen(function* () {
        const fake = yield* WsFake.make
        const conn = new WsMock.WebsocketConnection("wss://test", undefined)
        const loss = { tag: "session-lost" }
        yield* fake.setResponder(() => conn)
        yield* Effect.scoped(
          Effect.gen(function* () {
            const socket = yield* Websocket.connect("wss://test", {
              reconstructionPolicy: "report-connection-loss",
              closeCodeIsError: () => false,
            })
            if (operation === "receive") {
              conn.receive = () => Promise.reject(loss)
              const error = yield* Effect.flip(socket.runString<void, never, never>(() => {}))
              expect(Websocket.isSessionLost(error)).toBe(true)
              const encoded = yield* Schema.encodeEffect(Socket.SocketError)(error)
              const decoded = yield* Schema.decodeEffect(Socket.SocketError)(encoded)
              expect(Websocket.isSessionLost(decoded)).toBe(true)
              if (Websocket.isSessionLost(decoded))
                expect(decoded.reason.cause).toEqual({ _tag: "SessionLost" })
              return
            }
            const reader = yield* Effect.forkChild(socket.runString<void, never, never>(() => {}))
            if (operation === "send")
              conn.send = () => {
                throw loss
              }
            else
              conn.close = () => {
                throw loss
              }
            const writer = yield* socket.writer
            const error = yield* Effect.flip(
              writer.write(operation === "send" ? "hello" : new Socket.CloseEvent(1000)),
            )
            expect(Websocket.isSessionLost(error)).toBe(true)
            const encoded = yield* Schema.encodeEffect(Socket.SocketError)(error)
            const decoded = yield* Schema.decodeEffect(Socket.SocketError)(encoded)
            expect(Websocket.isSessionLost(decoded)).toBe(true)
            if (Websocket.isSessionLost(decoded))
              expect(decoded.reason.cause).toEqual({ _tag: "SessionLost" })
            if (operation === "close") {
              const readError = yield* Effect.flip(Fiber.join(reader))
              expect(readError).toBe(error)
            }
          }),
        ).pipe(Effect.provide(fake.layer))
        expect((yield* fake.recordedConnects)[0]?.reconstructionPolicy).toBe(
          "report-connection-loss",
        )
      }),
    )
  }
})
