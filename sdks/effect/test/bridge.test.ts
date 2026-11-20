import { describe, expect, it } from "@effect/vitest"
import { vi } from "vitest"
import { Effect, Fiber, Stream } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { AgentId } from "../src/AgentId.js"
import * as Bridge from "../src/Bridge.js"
import { RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"
import { agentIdFor, hostAgentIdFor, hostInvocationMetadata } from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

const graph: Bridge.SchemaGraph = { defs: new Map(), root: Bridge.t.string() }

describe("Bridge", () => {
  it.effect("maps upsert, create and getById to the host and releases on failure", () =>
    Effect.gen(function* () {
      const drop = vi.fn()
      const connection = (id: CoreTypes.AgentId) =>
        ({ agentId: () => Promise.resolve(id), drop }) as unknown as RpcConnection
      const opens: Array<unknown> = []
      const services = rpcLayer({
        upsert: (agentType, constructor) => {
          opens.push(["upsert", agentType, constructor])
          return Effect.succeed(connection(hostAgentIdFor("upserted")))
        },
        create: (agentType, constructor, options) => {
          opens.push(["create", agentType, constructor, options])
          return Effect.succeed({
            connection: connection(hostAgentIdFor("created")),
            agentId: hostAgentIdFor("created"),
          })
        },
        getById: (agentType, id) => {
          opens.push(["getById", agentType, id])
          return Effect.succeed(connection(id))
        },
      })
      const upsert = Bridge.upsertRemoteAgent("Example", Bridge.v.record([]))
      expect(opens).toEqual([])
      const ids = yield* Effect.scoped(
        Effect.gen(function* () {
          const upserted = yield* upsert
          const created = yield* Bridge.createRemoteAgent("Example", Bridge.v.record([]), {
            keyed: true,
            configs: [{ path: ["mode"], value: { graph, value: Bridge.v.string("fast") } }],
            env: { LEVEL: "debug" },
          })
          const found = yield* Bridge.getRemoteAgentById("Example", agentIdFor("existing"))
          return [yield* upserted.agentId, yield* created.agentId, yield* found.agentId]
        }),
      ).pipe(Effect.provide(services))
      expect(ids).toEqual([agentIdFor("upserted"), agentIdFor("created"), agentIdFor("existing")])
      expect(opens.map((open) => (open as Array<unknown>).slice(0, 2))).toEqual([
        ["upsert", "Example"],
        ["create", "Example"],
        ["getById", "Example"],
      ])
      expect(opens[1]).toMatchObject([
        "create",
        "Example",
        { valueNodes: [{ tag: "record-value" }] },
        {
          keyed: true,
          agentConfig: [
            {
              path: ["mode"],
              value: { value: { valueNodes: [{ tag: "string-value", val: "fast" }] } },
            },
          ],
          env: [["LEVEL", "debug"]],
        },
      ])
      expect(opens[2]).toEqual(["getById", "Example", hostAgentIdFor("existing")])
      expect(drop).toHaveBeenCalledTimes(3)

      const failed = yield* Effect.scoped(
        Effect.gen(function* () {
          yield* upsert
          return yield* Effect.fail("caller failed")
        }),
      ).pipe(Effect.provide(services), Effect.result)
      expect(failed).toMatchObject({ _tag: "Failure", failure: "caller failed" })
      expect(drop).toHaveBeenCalledTimes(4)

      const plain = yield* Effect.scoped(
        Bridge.createRemoteAgent("Example", Bridge.v.record([])),
      ).pipe(Effect.provide(services))
      expect(yield* plain.agentId).toEqual(agentIdFor("created"))
      expect(opens[4]).toMatchObject([
        "create",
        "Example",
        {},
        { keyed: false, agentConfig: [], env: [] },
      ])

      const conflict: AgentHost.RpcError = { tag: "conflict", val: "the key Example() is current" }
      const rejected = yield* Bridge.createRemoteAgent("Example", Bridge.v.record([]), {
        keyed: true,
      }).pipe(
        Effect.scoped,
        Effect.provide(
          rpcLayer({ create: () => Effect.fail(new RpcHostError(conflict, "WasmRpc.create")) }),
        ),
        Effect.result,
      )
      expect(rejected).toMatchObject({
        _tag: "Failure",
        failure: { _tag: "RpcCallError", cause: conflict },
      })
      if (rejected._tag === "Failure" && rejected.failure._tag === "RpcCallError")
        expect(rejected.failure.cause).toBe(conflict)

      const invalid = yield* Bridge.getRemoteAgentById("Example", {
        uuid: "bad",
      } as AgentId).pipe(Effect.scoped, Effect.provide(services), Effect.result)
      expect(invalid).toMatchObject({ _tag: "Failure", failure: { _tag: "InvalidUuidError" } })
      expect(opens).toHaveLength(5)
      expect(drop).toHaveBeenCalledTimes(5)
    }),
  )

  it.live("keeps resolution and invocation lazy and cancels an interrupted await", () => {
    let connects = 0
    let invokes = 0
    let cancels = 0
    let drops = 0
    const connection: RpcConnection = {
      agentId: () => Promise.resolve(hostAgentIdFor("example")),
      invokeAndAwait: () => ({ metadata: hostInvocationMetadata("agent") }),
      invoke: () => hostInvocationMetadata("agent"),
      asyncInvokeAndAwait: () => {
        invokes++
        return {
          metadata: hostInvocationMetadata("agent"),
          get: () => new Promise(() => undefined),
          cancel: () => cancels++,
          drop: () => drops++,
        }
      },
      scheduleInvocation: () => ({ metadata: hostInvocationMetadata("agent") }),
      scheduleCancelableInvocation: () => ({
        metadata: hostInvocationMetadata("agent"),
        token: { cancel: () => undefined, drop: () => undefined },
      }),
      drop: () => undefined,
    }
    const rpc = rpcLayer({
      upsert: () => {
        connects++
        return Effect.succeed(connection)
      },
    })
    const unresolved = Bridge.upsertRemoteAgent("Example", { tag: "record", fields: [] })
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
    ).pipe(Effect.provide(rpc)) as Effect.Effect<void, unknown, never>
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
