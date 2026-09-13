import { describe, expect, it } from "@effect/vitest"
import { Effect, Fiber, Layer, Result } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { fromAgentId } from "../src/DynamicClient.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { DurabilityModeClient } from "../src/host/DurabilityModeClient.js"
import { RpcClient, type RpcConnection } from "../src/host/RpcClient.js"
import { getAgentType, getAgentTypeByAgentId, getAllAgentTypes } from "../src/Reflection.js"
import { SchemaRef } from "../src/SchemaRef.js"
import { emptyMetadata, field, t, type SchemaGraph } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"

const metadata = emptyMetadata()
const recursive: SchemaGraph = {
  defs: new Map([
    [
      "node",
      {
        name: "Node",
        body: t.record([field("label", t.string()), field("next", t.option(t.ref("node")))]),
      },
    ],
  ]),
  root: t.ref("node"),
}

const registration = (): AgentHost.RegisteredAgentType => {
  const schema = schemaGraphToWit(recursive)
  const stringIndex = schema.typeNodes.length
  schema.typeNodes.push({ body: { tag: "string-type" }, metadata })
  return {
    implementedBy: { uuid: { highBits: 1n, lowBits: 2n } },
    agentType: {
      typeName: "Recursive",
      description: "recursive reflected agent",
      sourceLanguage: "typescript",
      mode: "durable",
      schema,
      constructor: {
        description: "constructor",
        inputSchema: {
          tag: "parameters",
          val: [{ name: "name", source: { tag: "user-supplied" }, schema: stringIndex, metadata }],
        },
      },
      methods: [
        {
          name: "walk",
          description: "walk",
          httpEndpoint: [],
          inputSchema: {
            tag: "parameters",
            val: [
              { name: "node", source: { tag: "user-supplied" }, schema: schema.root, metadata },
            ],
          },
          outputSchema: { tag: "single", val: stringIndex },
        },
      ],
      dependencies: [],
      snapshotting: { tag: "disabled" },
      config: [],
    },
  }
}

const hostLayer = (registered?: AgentHost.RegisteredAgentType) =>
  Layer.succeed(
    AgentHostClient,
    AgentHostClient.of({
      getAgentType: () => registered,
      getAgentTypeByAgentId: () => registered,
      getAllAgentTypes: () => (registered === undefined ? [] : [registered]),
    } as never),
  )

describe("reflection", () => {
  it("packs, unpacks, validates, renders, and deeply freezes recursive schemas", () => {
    const ref = new SchemaRef(schemaGraphToWit(recursive))
    const json = { label: "first", next: { label: "last", next: null } }
    expect(ref.unpackJson(ref.packJson(json))).toEqual(json)
    expect(ref.validateJson({ label: "missing-next" })).toMatchObject({ success: false })
    expect(ref.validateValue(ref.packJson(json))).toMatchObject({ success: true })
    expect(ref.toJsonSchema()).toMatchObject({ $schema: expect.any(String) })
    expect(Object.isFrozen(ref.graph)).toBe(true)
    expect(() => (ref.graph.defs as Map<string, unknown>).set("bad", {})).toThrow("immutable")
  })

  it.effect("returns optional discovery and concrete reflected schemas", () =>
    Effect.gen(function* () {
      expect(yield* getAgentType("missing")).toBeUndefined()
      const found = yield* getAgentType("Recursive").pipe(Effect.provide(hostLayer(registration())))
      expect(found?.implementedBy.uuid.highBits).toBe(1n)
      expect(
        found?.method("walk")?.input.validateJson({ node: { label: "x", next: null } }),
      ).toMatchObject({ success: true })
      expect(Object.isFrozen(found?.methods)).toBe(true)
    }).pipe(Effect.provide(hostLayer())),
  )

  it.effect("discovers by identity and enumerates types without opening RPC connections", () =>
    Effect.gen(function* () {
      expect(yield* getAgentTypeByAgentId("missing")).toBeUndefined()
      expect(yield* getAllAgentTypes).toEqual([])
      const host = Layer.succeed(AgentHostClient, {
        getAgentTypeByAgentId: (id: string) => {
          expect(id).toBe('Recursive("instance")')
          return registration()
        },
        getAllAgentTypes: () => [registration()],
      } as never)
      const found = yield* getAgentTypeByAgentId('Recursive("instance")').pipe(Effect.provide(host))
      expect(found?.name).toBe("Recursive")
      expect(found?.method("walk")?.name).toBe("walk")
      const all = yield* getAllAgentTypes.pipe(Effect.provide(host))
      expect(all.map((type) => type.name)).toEqual(["Recursive"])
      expect(Object.isFrozen(all)).toBe(true)
    }).pipe(Effect.provide(hostLayer())),
  )

  it.effect("uses lifecycle factories without manufacturing ephemeral identities", () => {
    const raw = registration()
    raw.agentType.mode = "ephemeral"
    const connections: Array<CoreTypes.Uuid | undefined> = []
    let dropped = 0
    const rpc = Layer.succeed(
      RpcClient,
      RpcClient.of({
        connect: (name, input, phantom) =>
          Effect.sync(() => {
            expect(name).toBe("Recursive")
            expect(input.valueNodes).toContainEqual({ tag: "string-value", val: "unique" })
            connections.push(phantom)
            return {
              drop: () => {
                dropped++
              },
            } as RpcConnection
          }),
      }),
    )
    const durability = Layer.succeed(DurabilityModeClient, {
      generateIdempotencyKey: () => ({ highBits: 1n, lowBits: 7n }),
    } as never)
    const host = Layer.succeed(AgentHostClient, {
      getAgentType: () => raw,
      makeAgentId: () => "durable-phantom",
    } as never)
    return Effect.gen(function* () {
      const ephemeral = (yield* getAgentType("Recursive"))!
      if (ephemeral.mode !== "ephemeral") throw new Error("expected ephemeral mode")
      expect("get" in ephemeral.client).toBe(false)
      expect("getPhantom" in ephemeral.client).toBe(false)
      yield* Effect.scoped(
        Effect.gen(function* () {
          const client = yield* ephemeral.client.newPhantom({ name: "unique" })
          expect("agentId" in client).toBe(false)
        }),
      )
      raw.agentType.mode = "durable"
      const durable = (yield* getAgentType("Recursive"))!
      if (durable.mode !== "durable") throw new Error("expected durable mode")
      yield* Effect.scoped(
        Effect.gen(function* () {
          const phantom = yield* durable.client.newPhantom({ name: "unique" })
          expect(phantom).toMatchObject({
            agentId: "durable-phantom",
            phantomId: "00000000-0000-0001-0000-000000000007",
          })
        }),
      )
      expect(connections).toEqual([undefined, { highBits: 1n, lowBits: 7n }])
      expect(dropped).toBe(2)
    }).pipe(Effect.provide(Layer.mergeAll(host, rpc, durability)))
  })

  it.effect(
    "validates reflected inputs and outputs and releases pending dynamic calls on interruption",
    () =>
      Effect.gen(function* () {
        const events: string[] = []
        let pending = false
        let output: CoreTypes.SchemaValueTree = {
          root: 0,
          valueNodes: [{ tag: "string-value", val: "answer" }],
        }
        const invocationMetadata = { agentId: "actual-id", idempotencyKey: "actual-key" }
        const connection: RpcConnection = {
          invokeAndAwait: () => {
            throw new Error("must use interruptible async call")
          },
          asyncInvokeAndAwait: () => ({
            metadata: invocationMetadata,
            get: () => {
              events.push("get")
              return pending ? new Promise(() => {}) : Promise.resolve(output)
            },
            cancel: () => {
              events.push("cancel")
            },
            drop: () => {
              events.push("future-drop")
            },
          }),
          invoke: () => invocationMetadata,
          scheduleInvocation: () => ({ metadata: invocationMetadata }),
          scheduleCancelableInvocation: () => ({
            metadata: invocationMetadata,
            token: {
              cancel: () => {
                events.push("token-cancel")
              },
              drop: () => {
                events.push("token-drop")
              },
            },
          }),
          drop: () => {
            events.push("connection-drop")
          },
        }
        const rpc = Layer.succeed(RpcClient, { connect: () => Effect.succeed(connection) })
        const host = Layer.succeed(AgentHostClient, {
          getAgentType: () => registration(),
          parseAgentId: () => ["Recursive", { value: { root: 0, valueNodes: [] } }, undefined],
        } as never)
        yield* Effect.scoped(
          Effect.gen(function* () {
            const type = (yield* getAgentType("Recursive"))!
            if (type.mode !== "durable") throw new Error("expected durable mode")
            const client = yield* type.client.get({ name: "unique" })
            const method = yield* client.method("walk")
            expect(Result.isFailure(yield* Effect.result(method.invoke({ node: {} })))).toBe(true)
            expect(events).toEqual([])
            expect(yield* method.invoke({ node: { label: "first", next: null } })).toEqual({
              metadata: invocationMetadata,
              value: "answer",
            })
            output = { root: 0, valueNodes: [{ tag: "bool-value", val: true }] }
            expect(
              yield* Effect.result(method.invoke({ node: { label: "first", next: null } })),
            ).toMatchObject({ _tag: "Failure", failure: { _tag: "RemoteOutputError" } })
            const scheduled = yield* method.schedule(
              { seconds: 123n, nanoseconds: 4 },
              { node: { label: "first", next: null } },
            )
            yield* scheduled.cancel
          }).pipe(Effect.provide(Layer.mergeAll(host, rpc))),
        )
        expect(events.slice(-3)).toEqual(["token-cancel", "token-drop", "connection-drop"])
        events.length = 0
        pending = true
        const fiber = yield* Effect.scoped(
          Effect.gen(function* () {
            const client = yield* fromAgentId("opaque-id")
            return yield* client.method("walk").invoke({ root: 0, valueNodes: [] })
          }),
        ).pipe(Effect.provide(Layer.mergeAll(host, rpc)), Effect.forkChild)
        while (!events.includes("get")) yield* Effect.yieldNow
        yield* Fiber.interrupt(fiber)
        expect(events).toEqual(["get", "cancel", "future-drop", "connection-drop"])
      }),
  )
})
