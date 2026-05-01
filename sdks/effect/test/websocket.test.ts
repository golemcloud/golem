import { describe, expect, it } from "@effect/vitest"
import { Effect, Fiber } from "effect"
import { Socket } from "effect/unstable/socket"
import * as Websocket from "../src/Websocket.js"
import * as WsFake from "./host/WsFake.js"
import * as WsMock from "./mocks/golem-websocket-client.js"

describe("Websocket.connect — error mapping", () => {
  it.effect("maps `connection-failure` to SocketError(SocketOpenError)", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      yield* fake.setResponder(() => {
        throw { tag: "connection-failure", val: "ECONNREFUSED" }
      })

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            yield* Websocket.connect("wss://test.example/echo")
          }),
        ),
      ).pipe(Effect.provide(fake.layer))

      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("SocketError")
        expect(json).toContain("SocketOpenError")
        expect(json).toContain("ECONNREFUSED")
      }
    }),
  )

  it.effect("issues close(1000, undefined) when the surrounding scope ends", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      let conn: WsMock.WebsocketConnection | undefined
      yield* fake.setResponder((url) => {
        conn = new WsMock.WebsocketConnection(url, undefined)
        return conn
      })

      yield* Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://test.example/echo")
        }),
      ).pipe(Effect.provide(fake.layer))

      expect(conn).toBeDefined()
      const closes = WsMock.__outboundCloses(conn!)
      expect(closes.length).toBe(1)
      expect(closes[0]?.code).toBe(1000)
    }),
  )
})

describe("Websocket — runString receives inbound text frames", () => {
  it.effect("delivers each text frame to the handler in order", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const seen: string[] = []
      yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo", {
            closeCodeIsError: (c) => c !== 1000,
          })
          // Pre-deliver three frames; the read loop will pick them up
          // and then the synthetic `closed` frame stops it cleanly.
          WsMock.__deliverInbound(conn, { tag: "text", val: "one" })
          WsMock.__deliverInbound(conn, { tag: "text", val: "two" })
          WsMock.__deliverInbound(conn, { tag: "text", val: "three" })
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })

          yield* sock.runString((line) => {
            seen.push(line)
          })
        }),
      ).pipe(Effect.provide(fake.layer))
      expect(seen).toEqual(["one", "two", "three"])
    }),
  )

  it.effect("decodes binary frames via runString", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const seen: string[] = []
      yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo", {
            closeCodeIsError: (c) => c !== 1000,
          })
          WsMock.__deliverInbound(conn, {
            tag: "binary",
            val: new TextEncoder().encode("bin-payload"),
          })
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* sock.runString((s) => {
            seen.push(s)
          })
        }),
      ).pipe(Effect.provide(fake.layer))
      expect(seen).toEqual(["bin-payload"])
    }),
  )

  it.effect("non-clean close codes surface as SocketError(SocketCloseError)", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const sock = yield* Websocket.connect("wss://test.example/echo")
            // Code 1006 (abnormal closure) — the default closeCodeIsError
            // (every code is an error) keeps this in the failure channel.
            WsMock.__signalClosed(conn, { code: 1006, reason: "abnormal" })
            yield* sock.runString(() => {})
          }),
        ),
      ).pipe(Effect.provide(fake.layer))

      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("SocketCloseError")
        expect(json).toContain("1006")
      }
    }),
  )

  it.effect("clean close codes are filtered out via closeCodeIsError", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const sock = yield* Websocket.connect("wss://test.example/echo", {
              closeCodeIsError: (code) => code !== 1000,
            })
            WsMock.__signalClosed(conn, { code: 1000, reason: "bye" })
            yield* sock.runString(() => {})
          }),
        ),
      ).pipe(Effect.provide(fake.layer))

      expect(exit._tag).toBe("Success")
    }),
  )
})

describe("Websocket — writer sends text/binary/close", () => {
  it.effect("text payloads round-trip through the writer", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo")
          // Drive the read loop in the background until close.
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          // Acquire the writer Effect inside its own scope so the
          // outer scope still owns the connection.
          yield* Effect.scoped(
            Effect.gen(function* () {
              const write = yield* sock.writer
              yield* write("hello")
              yield* write("world")
            }),
          )
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* Effect.exit(Fiber.join(fiber))
        }),
      ).pipe(Effect.provide(fake.layer))
      const out = WsMock.__outbound(conn)
      expect(out.length).toBe(2)
      expect(out[0]).toEqual({ tag: "text", val: "hello" })
      expect(out[1]).toEqual({ tag: "text", val: "world" })
    }),
  )

  it.effect("binary payloads round-trip through the writer", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo")
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          yield* Effect.scoped(
            Effect.gen(function* () {
              const write = yield* sock.writer
              yield* write(new TextEncoder().encode("ping"))
            }),
          )
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* Effect.exit(Fiber.join(fiber))
        }),
      ).pipe(Effect.provide(fake.layer))
      const out = WsMock.__outbound(conn)
      expect(out.length).toBe(1)
      expect(out[0]?.tag).toBe("binary")
      expect(new TextDecoder().decode((out[0] as { val: Uint8Array }).val)).toBe("ping")
    }),
  )

  it.effect("a CloseEvent triggers ws.close(...) and exits the read loop cleanly", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const sock = yield* Websocket.connect("wss://test.example/echo", {
              closeCodeIsError: (c) => c !== 1000,
            })
            const fiber = yield* Effect.forkChild(sock.runString(() => {}))
            yield* Effect.scoped(
              Effect.gen(function* () {
                const write = yield* sock.writer
                yield* write(new Socket.CloseEvent(1000, "client said bye"))
              }),
            )
            yield* Fiber.join(fiber)
          }),
        ),
      ).pipe(Effect.provide(fake.layer))
      expect(exit._tag).toBe("Success")
      const closes = WsMock.__outboundCloses(conn)
      expect(closes.length).toBeGreaterThanOrEqual(1)
      expect(closes[0]?.code).toBe(1000)
      expect(closes[0]?.reason).toBe("client said bye")
    }),
  )

  it.effect("send-failure during write surfaces as SocketError(SocketWriteError)", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      // Wire the send to throw a tagged WIT error.
      conn._sendImpl = () => {
        throw { tag: "send-failure", val: "queue is full" }
      }
      yield* fake.setResponder(() => conn)

      const writeExit = yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo")
          // Run the read loop; it stays blocked on subscribe() until close.
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          const writeExit = yield* Effect.exit(
            Effect.scoped(
              Effect.gen(function* () {
                const write = yield* sock.writer
                yield* write("payload")
              }),
            ),
          )
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* Effect.exit(Fiber.join(fiber))
          return writeExit
        }),
      ).pipe(Effect.provide(fake.layer))
      expect(writeExit._tag).toBe("Failure")
      if (writeExit._tag === "Failure") {
        const json = JSON.stringify(writeExit.cause)
        expect(json).toContain("SocketWriteError")
        expect(json).toContain("queue is full")
      }
    }),
  )
})

describe("Websocket — Layer integration", () => {
  it.effect("layer(url) provides a Socket service that can be consumed via Socket.Socket.use", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)
      WsMock.__deliverInbound(conn, { tag: "text", val: "from-layer" })
      WsMock.__signalClosed(conn, { code: 1000, reason: "" })

      const seen: string[] = []
      yield* Effect.gen(function* () {
        const sock = yield* Socket.Socket.asEffect()
        yield* sock.runString((s) => {
          seen.push(s)
        })
      }).pipe(
        Effect.provide(
          Websocket.layer("wss://test.example/echo", {
            closeCodeIsError: (c) => c !== 1000,
          }),
        ),
        Effect.provide(fake.layer),
      )
      expect(seen).toEqual(["from-layer"])
    }),
  )
})

describe("Websocket — makeChannel produces a Channel value", () => {
  it("makeChannel returns a Channel object (smoke test)", () => {
    const channel = Websocket.makeChannel<unknown>("wss://test.example/echo")
    expect(channel).toBeDefined()
    expect(typeof channel).toBe("object")
  })
})

describe("Websocket — call-site sensitive error classification", () => {
  it.effect("`other` thrown from connect maps to SocketOpenError, not SocketReadError", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      yield* fake.setResponder(() => {
        throw { tag: "other", val: "weird-handshake-state" }
      })

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            yield* Websocket.connect("wss://test.example/echo")
          }),
        ),
      ).pipe(Effect.provide(fake.layer))

      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("SocketOpenError")
        expect(json).not.toContain("SocketReadError")
        expect(json).toContain("weird-handshake-state")
      }
    }),
  )

  it.effect("`protocol-error` thrown from connect maps to SocketOpenError", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      yield* fake.setResponder(() => {
        throw { tag: "protocol-error", val: "bad-handshake" }
      })

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            yield* Websocket.connect("wss://test.example/echo")
          }),
        ),
      ).pipe(Effect.provide(fake.layer))

      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("SocketOpenError")
        expect(json).not.toContain("SocketReadError")
        expect(json).toContain("bad-handshake")
      }
    }),
  )

  it.effect("`other` thrown from send maps to SocketWriteError, not SocketReadError", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      conn._sendImpl = () => {
        throw { tag: "other", val: "weird-send-state" }
      }
      yield* fake.setResponder(() => conn)

      const writeExit = yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo")
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          const writeExit = yield* Effect.exit(
            Effect.scoped(
              Effect.gen(function* () {
                const write = yield* sock.writer
                yield* write("payload")
              }),
            ),
          )
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* Effect.exit(Fiber.join(fiber))
          return writeExit
        }),
      ).pipe(Effect.provide(fake.layer))
      expect(writeExit._tag).toBe("Failure")
      if (writeExit._tag === "Failure") {
        const json = JSON.stringify(writeExit.cause)
        expect(json).toContain("SocketWriteError")
        expect(json).not.toContain("SocketReadError")
        expect(json).toContain("weird-send-state")
      }
    }),
  )

  it.effect("`other` thrown from receive maps to SocketReadError", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const sock = yield* Websocket.connect("wss://test.example/echo")
            WsMock.__deliverError(conn, { tag: "other", val: "weird-recv-state" })
            yield* sock.runString(() => {})
          }),
        ),
      ).pipe(Effect.provide(fake.layer))
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("SocketReadError")
        expect(json).toContain("weird-recv-state")
      }
    }),
  )
})

describe("Websocket — read loop is interruption-safe", () => {
  // Real-time test: uses Effect.sleep + Fiber.interrupt + race-with-watchdog.
  it.live("interrupting the run-loop fiber wakes pollable.abortablePromise and exits cleanly", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      const program = Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo")
          // Fork the read loop. With NO inbound frames delivered the
          // loop is parked in pollable.abortablePromise(...). The
          // explicit Fiber.interrupt below must wake it up — if the
          // SDK still used the non-abortable `pollable.promise()` the
          // promise would resolve only once a frame arrived, leaking
          // the host pollable indefinitely.
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          // Yield once to make sure the read loop has actually entered
          // its `await pollable.abortablePromise(...)` call before we
          // interrupt.
          yield* Effect.sleep("10 millis")
          yield* Fiber.interrupt(fiber)
        }),
      ).pipe(Effect.provide(fake.layer))

      // Race against a watchdog: if interruption did not wake the read
      // loop the program would hang forever and this race would emit a
      // failure exit.
      const result = yield* Effect.race(
        program.pipe(Effect.map(() => "ok" as const)),
        Effect.sleep("2 seconds").pipe(Effect.map(() => "watchdog" as const)),
      )
      expect(result).toBe("ok")
    }),
  )
})

describe("Websocket — local CloseEvent signals local termination", () => {
  // Real-time test: uses watchdog race against Effect.sleep("2 seconds").
  it.live("CloseEvent terminates the read loop even if the host never wakes the pollable", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      // Override `close()` so it does NOT push a synthetic `closed`
      // error onto the inbound queue: this simulates a host that is
      // silent on local close. The SDK must still terminate runRaw
      // promptly because we explicitly fail the read fiber's deferred.
      conn.close = function (
        this: WsMock.WebsocketConnection,
        code: number | undefined,
        reason: string | undefined,
      ) {
        if (this._closed) return
        this._closed = true
        this._outboundCloses.push({ code, reason })
      }.bind(conn)
      yield* fake.setResponder(() => conn)

      const program = Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo", {
            closeCodeIsError: (c) => c !== 1000,
          })
          const fiber = yield* Effect.forkChild(sock.runString(() => {}))
          yield* Effect.scoped(
            Effect.gen(function* () {
              const write = yield* sock.writer
              yield* write(new Socket.CloseEvent(1000, "client said bye"))
            }),
          )
          // Should join cleanly: the local close failed the deferred
          // with a SocketCloseError(code=1000) which the
          // closeCodeIsError filter classifies as a clean shutdown.
          yield* Fiber.join(fiber)
        }),
      ).pipe(Effect.provide(fake.layer))

      const result = yield* Effect.race(
        program.pipe(Effect.map(() => "ok" as const)),
        Effect.sleep("2 seconds").pipe(Effect.map(() => "watchdog" as const)),
      )
      expect(result).toBe("ok")
      const closes = WsMock.__outboundCloses(conn)
      expect(closes[0]?.code).toBe(1000)
      expect(closes[0]?.reason).toBe("client said bye")
    }),
  )
})

describe("Websocket — writer-before-runRaw contract", () => {
  // Real-time test: uses Effect.sleep("50 millis") to verify the writer suspends.
  it.live("writer suspends until a run* call activates the latch", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
      yield* fake.setResponder(() => conn)

      yield* Effect.scoped(
        Effect.gen(function* () {
          const sock = yield* Websocket.connect("wss://test.example/echo", {
            closeCodeIsError: (c) => c !== 1000,
          })
          // Acquire the writer and try to send BEFORE forking runString.
          // The writer must suspend on the internal latch and not push
          // anything to the host yet.
          const writeFiber = yield* Effect.forkChild(
            Effect.scoped(
              Effect.gen(function* () {
                const write = yield* sock.writer
                yield* write("buffered")
              }),
            ),
          )
          yield* Effect.sleep("50 millis")
          // Latch never opened: nothing should have been sent yet.
          expect(WsMock.__outbound(conn).length).toBe(0)
          // Now fork the read loop; latch opens and the suspended
          // writer should make progress.
          const reader = yield* Effect.forkChild(sock.runString(() => {}))
          yield* Fiber.join(writeFiber)
          expect(WsMock.__outbound(conn).length).toBe(1)
          expect(WsMock.__outbound(conn)[0]).toEqual({ tag: "text", val: "buffered" })
          // Tear down read loop.
          WsMock.__signalClosed(conn, { code: 1000, reason: "" })
          yield* Fiber.join(reader)
        }),
      ).pipe(Effect.provide(fake.layer))
    }),
  )
})

describe("Websocket — WsFake invariants", () => {
  it.effect("recordedConnects captures every connect attempt (including failures)", () =>
    Effect.gen(function* () {
      const fake = yield* WsFake.make
      yield* fake.setNextConnectError({ tag: "connection-failure", val: "first attempt" })
      yield* fake.setResponder((url) => new WsMock.WebsocketConnection(url, undefined))

      // First attempt fails via the one-shot injector.
      const firstExit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            yield* Websocket.connect("wss://first.example/echo", {
              headers: [["x-trace", "first"]],
            })
          }),
        ),
      ).pipe(Effect.provide(fake.layer))
      expect(firstExit._tag).toBe("Failure")

      // Second attempt succeeds via the responder.
      yield* Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://second.example/echo")
        }),
      ).pipe(Effect.provide(fake.layer))

      const recorded = yield* fake.recordedConnects.pipe(Effect.provide(fake.layer))
      expect(recorded.length).toBe(2)
      expect(recorded[0]?.url).toBe("wss://first.example/echo")
      expect(recorded[0]?.headers).toEqual([["x-trace", "first"]])
      expect(recorded[1]?.url).toBe("wss://second.example/echo")
      expect(recorded[1]?.headers).toBeUndefined()
    }),
  )
})
