import { describe, expect, it } from "@effect/vitest"
import { vi } from "vitest"
import { Effect, Fiber, Layer, Stream } from "effect"
import * as Bridge from "../src/Bridge.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcClient, RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"

describe("Bridge", () => {
  it.effect(
    "prepares ephemeral connections without making reusable IDs and releases on failure",
    () =>
      Effect.gen(function* () {
        const makeAgentId = vi.fn(() => "unexpected")
        const drop = vi.fn()
        const connect = vi.fn(() => Effect.succeed({ drop } as unknown as RpcConnection))
        const services = Layer.merge(
          Layer.succeed(AgentHostClient, { makeAgentId } as never),
          Layer.succeed(RpcClient, { connect }),
        )
        const resolve = Bridge.resolveRemoteAgent(
          "Ephemeral",
          Bridge.v.record([]),
          undefined,
          [],
          "ephemeral",
        )
        expect(connect).not.toHaveBeenCalled()
        const result = yield* Effect.scoped(
          Effect.gen(function* () {
            const remote = yield* resolve
            expect(remote.agentId).toBe("Ephemeral")
            expect(drop).not.toHaveBeenCalled()
            return yield* Effect.fail("caller failed")
          }),
        ).pipe(Effect.provide(services), Effect.result)
        expect(result).toMatchObject({ _tag: "Failure", failure: "caller failed" })
        expect(makeAgentId).not.toHaveBeenCalled()
        expect(connect).toHaveBeenCalledTimes(1)
        expect(drop).toHaveBeenCalledTimes(1)

        const cause = { tag: "denied" as const, val: "denied by host" }
        const rejected = yield* resolve.pipe(
          Effect.scoped,
          Effect.provide(
            Layer.merge(
              Layer.succeed(AgentHostClient, { makeAgentId } as never),
              Layer.succeed(RpcClient, {
                connect: () => Effect.fail(new RpcHostError(cause, "WasmRpc.create")),
              }),
            ),
          ),
          Effect.result,
        )
        expect(rejected).toMatchObject({
          _tag: "Failure",
          failure: { _tag: "RpcCallError", cause },
        })
        if (rejected._tag === "Failure" && rejected.failure._tag === "RpcCallError")
          expect(rejected.failure.cause).toBe(cause)
        expect(drop).toHaveBeenCalledTimes(1)
      }),
  )

  it.live("keeps resolution and invocation lazy and cancels an interrupted await", () => {
    let connects = 0
    let invokes = 0
    let cancels = 0
    let drops = 0
    const connection: RpcConnection = {
      invokeAndAwait: () => ({ metadata: { agentId: "agent", idempotencyKey: "sync" } }),
      invoke: () => ({ agentId: "agent", idempotencyKey: "trigger" }),
      asyncInvokeAndAwait: () => {
        invokes++
        return {
          metadata: { agentId: "agent", idempotencyKey: "await" },
          get: () => new Promise(() => undefined),
          cancel: () => cancels++,
          drop: () => drops++,
        }
      },
      scheduleInvocation: () => ({ metadata: { agentId: "agent", idempotencyKey: "schedule" } }),
      scheduleCancelableInvocation: () => ({
        metadata: { agentId: "agent", idempotencyKey: "schedule" },
        token: { cancel: () => undefined, drop: () => undefined },
      }),
      drop: () => undefined,
    }
    const rpc = Layer.succeed(
      RpcClient,
      RpcClient.of({
        connect: () => {
          connects++
          return Effect.succeed(connection)
        },
      }),
    )
    const agents = Layer.succeed(
      AgentHostClient,
      AgentHostClient.of({
        makeAgentId: () => "Example()",
      } as never),
    )
    const unresolved = Bridge.resolveRemoteAgent(
      "Example",
      { tag: "record", fields: [] },
      undefined,
      [],
      "durable",
    )
    expect(connects).toBe(0)
    return Effect.scoped(
      Effect.gen(function* () {
        const handle = yield* unresolved
        expect(connects).toBe(1)
        const fiber = yield* Effect.forkChild(
          handle.invokeAndAwait("wait", { tag: "record", fields: [] }, (value) => value),
        )
        yield* Effect.sleep("1 millis")
        expect(invokes).toBe(1)
        yield* Fiber.interrupt(fiber)
        expect(cancels).toBe(1)
        expect(drops).toBe(1)
      }),
    ).pipe(Effect.provide(Layer.merge(rpc, agents))) as Effect.Effect<void, unknown, never>
  })

  it("preserves a nested stream item's generated codec", async () => {
    const codec: Bridge.SchemaCodec<readonly number[]> = {
      graph: { defs: new Map(), root: Bridge.t.list(Bridge.t.u32()) },
      toValue: (values) => Bridge.v.list(values.map(Bridge.v.u32)),
      fromValue: (value) => {
        if (value.tag !== "list") throw new Error("expected list")
        return value.elements.map((item) => {
          if (item.tag !== "u32") throw new Error("expected u32")
          return item.value
        })
      },
    }
    const source = Stream.make([1, 2], [3])
    const forwarded = Bridge.agentStreamFromHandle(Bridge.agentStreamToHandle(source, codec), codec)
    expect([...(await Effect.runPromise(Stream.runCollect(forwarded)))]).toEqual([[1, 2], [3]])
  })

  it("validates protocol-shaped helper input instead of silently coercing it", () => {
    expect(() => Bridge.decodeOption(Bridge.v.string("wrong"), String)).toThrow("Expected option")
    expect(() => Bridge.datetimeFromISOString("2026-02-30T00:00:00Z")).toThrow("Invalid datetime")
  })

  it("closes unread stream siblings when generated result decoding fails", async () => {
    let closed = 0
    const wire = {
      root: 2,
      valueNodes: [
        { tag: "stream-value", val: { [Symbol.dispose]: () => closed++ } },
        { tag: "u32-value", val: 17 },
        { tag: "record-value", val: [0, 1] },
      ],
    } as Parameters<typeof Bridge.decodeWire>[0]
    expect(() =>
      Bridge.decodeWire(wire, () => {
        throw new Error("invalid generated result")
      }),
    ).toThrow("invalid generated result")
    await vi.waitFor(() => expect(closed).toBe(1))
    expect(wire?.valueNodes[0]).toEqual({ tag: "stream-value", val: undefined })
  })
})
