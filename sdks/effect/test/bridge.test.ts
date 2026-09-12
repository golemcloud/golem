import { describe, expect, it } from "@effect/vitest"
import { Effect, Fiber, Layer } from "effect"
import * as Bridge from "../src/Bridge.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcClient, type RpcConnection } from "../src/host/RpcClient.js"

describe("Bridge", () => {
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
    const source = Bridge.AgentStream.from([[1, 2], [3]])
    const forwarded = Bridge.agentStreamFromHandle(Bridge.agentStreamToHandle(source, codec), codec)
    expect(await forwarded.next()).toEqual({ done: false, value: [1, 2] })
    expect(await forwarded.next()).toEqual({ done: false, value: [3] })
    expect(await forwarded.next()).toEqual({ done: true, value: undefined })
  })

  it("validates protocol-shaped helper input instead of silently coercing it", () => {
    expect(() => Bridge.decodeOption(Bridge.v.string("wrong"), String)).toThrow("Expected option")
    expect(() => Bridge.datetimeFromISOString("2026-02-30T00:00:00Z")).toThrow("Invalid datetime")
  })

  it("closes unread stream siblings when generated result decoding fails", () => {
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
    expect(closed).toBe(1)
    expect(wire?.valueNodes[0]).toEqual({ tag: "stream-value", val: undefined })
  })
})
