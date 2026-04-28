import { Effect, Exit, Fiber } from "effect"
import { Socket } from "effect/unstable/socket"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Websocket from "../src/websocket.js"
import * as WsMock from "./mocks/golem-websocket-client.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

beforeEach(() => {
  WsMock.__resetAll()
  Websocket.__resetConnectForTest()
})
afterEach(() => {
  WsMock.__resetAll()
  Websocket.__resetConnectForTest()
})

describe("Websocket.connect — error mapping", () => {
  it("maps `connection-failure` to SocketError(SocketOpenError)", async () => {
    WsMock.__setConnectImpl(() => {
      throw { tag: "connection-failure", val: "ECONNREFUSED" }
    })

    const exit = await runExit(
      Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://test.example/echo")
        }),
      ),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("SocketError")
      expect(json).toContain("SocketOpenError")
      expect(json).toContain("ECONNREFUSED")
    }
  })

  it("issues close(1000, undefined) when the surrounding scope ends", async () => {
    let conn: WsMock.WebsocketConnection | undefined
    WsMock.__setConnectImpl((url) => {
      conn = new WsMock.WebsocketConnection(url, undefined)
      return conn
    })

    await runP(
      Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://test.example/echo")
        }),
      ),
    )

    expect(conn).toBeDefined()
    const closes = WsMock.__outboundCloses(conn!)
    expect(closes.length).toBe(1)
    expect(closes[0]?.code).toBe(1000)
  })
})

describe("Websocket — runString receives inbound text frames", () => {
  it("delivers each text frame to the handler in order", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const seen: string[] = []
    const program = Effect.scoped(
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
    )

    await runP(program)
    expect(seen).toEqual(["one", "two", "three"])
  })

  it("decodes binary frames via runString", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const seen: string[] = []
    const program = Effect.scoped(
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
    )

    await runP(program)
    expect(seen).toEqual(["bin-payload"])
  })

  it("non-clean close codes surface as SocketError(SocketCloseError)", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
      Effect.gen(function* () {
        const sock = yield* Websocket.connect("wss://test.example/echo")
        // Code 1006 (abnormal closure) — the default closeCodeIsError
        // (every code is an error) keeps this in the failure channel.
        WsMock.__signalClosed(conn, { code: 1006, reason: "abnormal" })
        yield* sock.runString(() => {})
      }),
    )

    const exit = await runExit(program)
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("SocketCloseError")
      expect(json).toContain("1006")
    }
  })

  it("clean close codes are filtered out via closeCodeIsError", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
      Effect.gen(function* () {
        const sock = yield* Websocket.connect("wss://test.example/echo", {
          closeCodeIsError: (code) => code !== 1000,
        })
        WsMock.__signalClosed(conn, { code: 1000, reason: "bye" })
        yield* sock.runString(() => {})
      }),
    )

    const exit = await runExit(program)
    expect(exit._tag).toBe("Success")
  })
})

describe("Websocket — writer sends text/binary/close", () => {
  it("text payloads round-trip through the writer", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
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
    )

    await runP(program)
    const out = WsMock.__outbound(conn)
    expect(out.length).toBe(2)
    expect(out[0]).toEqual({ tag: "text", val: "hello" })
    expect(out[1]).toEqual({ tag: "text", val: "world" })
  })

  it("binary payloads round-trip through the writer", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
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
    )

    await runP(program)
    const out = WsMock.__outbound(conn)
    expect(out.length).toBe(1)
    expect(out[0]?.tag).toBe("binary")
    expect(new TextDecoder().decode((out[0] as { val: Uint8Array }).val)).toBe("ping")
  })

  it("a CloseEvent triggers ws.close(...) and exits the read loop cleanly", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

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
        yield* Fiber.join(fiber)
      }),
    )

    const exit = await runExit(program)
    expect(exit._tag).toBe("Success")
    const closes = WsMock.__outboundCloses(conn)
    expect(closes.length).toBeGreaterThanOrEqual(1)
    expect(closes[0]?.code).toBe(1000)
    expect(closes[0]?.reason).toBe("client said bye")
  })

  it("send-failure during write surfaces as SocketError(SocketWriteError)", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    // Wire the send to throw a tagged WIT error.
    conn._sendImpl = () => {
      throw { tag: "send-failure", val: "queue is full" }
    }
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
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
    )

    const writeExit = await runP(program)
    expect(writeExit._tag).toBe("Failure")
    if (writeExit._tag === "Failure") {
      const json = JSON.stringify(writeExit.cause)
      expect(json).toContain("SocketWriteError")
      expect(json).toContain("queue is full")
    }
  })
})

describe("Websocket — Layer integration", () => {
  it("layer(url) provides a Socket service that can be consumed via Socket.Socket.use", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)
    WsMock.__deliverInbound(conn, { tag: "text", val: "from-layer" })
    WsMock.__signalClosed(conn, { code: 1000, reason: "" })

    const seen: string[] = []
    const program = Effect.gen(function* () {
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
    )

    await runP(program)
    expect(seen).toEqual(["from-layer"])
  })
})

describe("Websocket — makeChannel produces a Channel value", () => {
  it("makeChannel returns a Channel object (smoke test)", () => {
    const channel = Websocket.makeChannel<unknown>("wss://test.example/echo")
    expect(channel).toBeDefined()
    expect(typeof channel).toBe("object")
  })
})

describe("Websocket — call-site sensitive error classification", () => {
  it("`other` thrown from connect maps to SocketOpenError, not SocketReadError", async () => {
    WsMock.__setConnectImpl(() => {
      throw { tag: "other", val: "weird-handshake-state" }
    })

    const exit = await runExit(
      Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://test.example/echo")
        }),
      ),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("SocketOpenError")
      expect(json).not.toContain("SocketReadError")
      expect(json).toContain("weird-handshake-state")
    }
  })

  it("`protocol-error` thrown from connect maps to SocketOpenError", async () => {
    WsMock.__setConnectImpl(() => {
      throw { tag: "protocol-error", val: "bad-handshake" }
    })

    const exit = await runExit(
      Effect.scoped(
        Effect.gen(function* () {
          yield* Websocket.connect("wss://test.example/echo")
        }),
      ),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("SocketOpenError")
      expect(json).not.toContain("SocketReadError")
      expect(json).toContain("bad-handshake")
    }
  })

  it("`other` thrown from send maps to SocketWriteError, not SocketReadError", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    conn._sendImpl = () => {
      throw { tag: "other", val: "weird-send-state" }
    }
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
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
    )

    const writeExit = await runP(program)
    expect(writeExit._tag).toBe("Failure")
    if (writeExit._tag === "Failure") {
      const json = JSON.stringify(writeExit.cause)
      expect(json).toContain("SocketWriteError")
      expect(json).not.toContain("SocketReadError")
      expect(json).toContain("weird-send-state")
    }
  })

  it("`other` thrown from receive maps to SocketReadError", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
      Effect.gen(function* () {
        const sock = yield* Websocket.connect("wss://test.example/echo")
        WsMock.__deliverError(conn, { tag: "other", val: "weird-recv-state" })
        yield* sock.runString(() => {})
      }),
    )

    const exit = await runExit(program)
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("SocketReadError")
      expect(json).toContain("weird-recv-state")
    }
  })
})

describe("Websocket — read loop is interruption-safe", () => {
  it("interrupting the run-loop fiber wakes pollable.abortablePromise and exits cleanly", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

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
    )

    // Race against a watchdog: if interruption did not wake the read
    // loop the program would hang forever and this race would emit a
    // failure exit.
    const watched = Effect.race(
      program.pipe(Effect.map(() => "ok" as const)),
      Effect.sleep("2 seconds").pipe(Effect.map(() => "watchdog" as const)),
    )

    const result = await runP(watched)
    expect(result).toBe("ok")
  })
})

describe("Websocket — local CloseEvent signals local termination", () => {
  it("CloseEvent terminates the read loop even if the host never wakes the pollable", async () => {
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
    WsMock.__setConnectImpl(() => conn)

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
    )

    const watched = Effect.race(
      program.pipe(Effect.map(() => "ok" as const)),
      Effect.sleep("2 seconds").pipe(Effect.map(() => "watchdog" as const)),
    )
    const result = await runP(watched)
    expect(result).toBe("ok")
    const closes = WsMock.__outboundCloses(conn)
    expect(closes[0]?.code).toBe(1000)
    expect(closes[0]?.reason).toBe("client said bye")
  })
})

describe("Websocket — writer-before-runRaw contract", () => {
  it("writer suspends until a run* call activates the latch", async () => {
    const conn = new WsMock.WebsocketConnection("wss://test.example/echo", undefined)
    WsMock.__setConnectImpl(() => conn)

    const program = Effect.scoped(
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
    )

    await runP(program)
  })
})
