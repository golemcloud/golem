import { describe, expect, it } from "@effect/vitest"
import { Effect, Fiber, Layer, Result } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as Client from "../src/Client.js"
import { bind as bindDynamic } from "../src/DynamicClient.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"
import { getAgentType, getAgentTypeByAgentId, getAllAgentTypes } from "../src/Reflection.js"
import { SchemaRef } from "../src/SchemaRef.js"
import { emptyMetadata, field, t, type SchemaGraph } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"
import { createScopeCard } from "../src/internal/scopeCard.js"
import {
  agentIdFor,
  hostAgentIdFor,
  hostInvocationMetadata,
  sdkInvocationMetadata,
} from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

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
    agentType: {
      typeName: "Recursive",
      kind: "regular",
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
    expect(ref.validateJson({ label: "missing-next" })).toMatchObject({ success: true })
    expect(ref.unpackJson(ref.packJson({ label: "missing-next" }))).toEqual({
      label: "missing-next",
      next: null,
    })
    expect(ref.validateValue(ref.packJson(json))).toMatchObject({ success: true })
    expect(ref.toJsonSchema()).toMatchObject({ $schema: expect.any(String) })
    expect(Object.isFrozen(ref.graph)).toBe(true)
    expect(() => (ref.graph.defs as Map<string, unknown>).set("bad", {})).toThrow("immutable")
  })

  it.effect("returns optional discovery and concrete reflected schemas", () =>
    Effect.gen(function* () {
      expect(yield* getAgentType("missing")).toBeUndefined()
      const found = yield* getAgentType("Recursive").pipe(Effect.provide(hostLayer(registration())))
      expect(found?.name).toBe("Recursive")
      expect(
        found?.method("walk")?.input.validateJson({ node: { label: "x", next: null } }),
      ).toMatchObject({ success: true })
      expect(Object.isFrozen(found?.methods)).toBe(true)
      expect(found?.constructorInput.graph.defs).toBe(found?.method("walk")?.input.graph.defs)
      expect(found?.constructorInput.graph.root).toBe(found?.constructorInput.root)
      expect(found?.method("walk")?.output?.graph.root).toBe(found?.method("walk")?.output?.root)
      expect(found?.constructorInput.graph.defs).toBe(found?.method("walk")?.output?.graph.defs)
    }).pipe(Effect.provide(hostLayer())),
  )

  it.effect("surfaces discovery traps and malformed schema graphs as typed failures", () =>
    Effect.gen(function* () {
      const trapped = Layer.succeed(AgentHostClient, {
        getAgentType: () => {
          throw new Error("host unavailable")
        },
      } as never)
      expect(
        yield* getAgentType("Recursive").pipe(Effect.provide(trapped), Effect.result),
      ).toMatchObject({ _tag: "Failure", failure: { _tag: "ReflectionHostError" } })

      const malformed = registration()
      malformed.agentType.methods[0]!.outputSchema = { tag: "single", val: 999 }
      expect(
        yield* getAgentType("Recursive").pipe(Effect.provide(hostLayer(malformed)), Effect.result),
      ).toMatchObject({ _tag: "Failure", failure: { _tag: "ReflectionSchemaError" } })
    }),
  )

  it.effect("discovers by agent ID and enumerates types without opening RPC connections", () =>
    Effect.gen(function* () {
      expect(yield* getAgentTypeByAgentId(agentIdFor("missing"))).toBeUndefined()
      expect(yield* getAllAgentTypes).toEqual([])
      const host = Layer.succeed(AgentHostClient, {
        getAgentTypeByAgentId: (id: CoreTypes.AgentId) => {
          expect(id).toEqual(hostAgentIdFor("instance"))
          return registration()
        },
        getAllAgentTypes: () => [registration()],
      } as never)
      const found = yield* getAgentTypeByAgentId(agentIdFor("instance")).pipe(Effect.provide(host))
      expect(found?.name).toBe("Recursive")
      expect(found?.method("walk")?.name).toBe("walk")
      const all = yield* getAllAgentTypes.pipe(Effect.provide(host))
      expect(all.map((type) => type.name)).toEqual(["Recursive"])
      expect(Object.isFrozen(all)).toBe(true)
    }).pipe(Effect.provide(hostLayer())),
  )

  it.effect("maps lifecycle factories to upsert, create and getById by mode", () => {
    const raw = registration()
    raw.agentType.mode = "ephemeral"
    const opens: Array<readonly [string, string, unknown]> = []
    let dropped = 0
    const connection = (id: CoreTypes.AgentId) =>
      ({
        agentId: () => Promise.resolve(id),
        drop: () => {
          dropped++
        },
      }) as unknown as RpcConnection
    const rpc = rpcLayer({
      upsert: (name, input) =>
        Effect.sync(() => {
          expect(input.valueNodes).toContainEqual({ tag: "string-value", val: "unique" })
          opens.push(["upsert", name, undefined])
          return connection(hostAgentIdFor("upserted"))
        }),
      create: (name, input, options) =>
        Effect.sync(() => {
          expect(input.valueNodes).toContainEqual({ tag: "string-value", val: "unique" })
          opens.push(["create", name, options])
          return {
            connection: connection(hostAgentIdFor("created")),
            agentId: hostAgentIdFor("created"),
          }
        }),
      getById: (name, id) =>
        Effect.sync(() => {
          opens.push(["getById", name, id])
          return connection(id)
        }),
    })
    const host = Layer.succeed(AgentHostClient, {
      getAgentType: () => raw,
    } as never)
    return Effect.gen(function* () {
      const ephemeral = (yield* getAgentType("Recursive"))!
      if (ephemeral.mode !== "ephemeral") throw new Error("expected ephemeral mode")
      expect(Object.keys(ephemeral.client).sort()).toEqual(["upsert", "upsertValue"])
      yield* Effect.scoped(
        Effect.gen(function* () {
          const client = yield* ephemeral.client.upsert({ name: "unique" })
          expect("agentId" in client).toBe(false)
        }),
      )
      expect(
        yield* Client.bind(agentIdFor("ephemeral"), ephemeral).pipe(Effect.scoped, Effect.result),
      ).toMatchObject({ _tag: "Failure", failure: { _tag: "ClientBindingError" } })

      raw.agentType.mode = "durable"
      const durable = (yield* getAgentType("Recursive"))!
      if (durable.mode !== "durable") throw new Error("expected durable mode")
      const ids = yield* Effect.scoped(
        Effect.gen(function* () {
          const upserted = yield* durable.client.upsert({ name: "unique" })
          const created = yield* durable.client.create({ name: "unique" })
          const keyed = yield* durable.client.create(
            { name: "unique" },
            { keyed: true, env: { MODE: "test" } },
          )
          const found = yield* durable.client.getById(agentIdFor("existing"))
          const bound = yield* Client.bind(agentIdFor("bound"), durable)
          return yield* Effect.all([
            upserted.agentId,
            created.agentId,
            keyed.agentId,
            found.agentId,
            bound.agentId,
          ])
        }),
      )
      expect(ids).toEqual([
        agentIdFor("upserted"),
        agentIdFor("created"),
        agentIdFor("created"),
        agentIdFor("existing"),
        agentIdFor("bound"),
      ])
      expect(opens).toEqual([
        ["upsert", "Recursive", undefined],
        ["upsert", "Recursive", undefined],
        ["create", "Recursive", { keyed: false, agentConfig: [], env: [] }],
        ["create", "Recursive", { keyed: true, agentConfig: [], env: [["MODE", "test"]] }],
        ["getById", "Recursive", hostAgentIdFor("existing")],
        ["getById", "Recursive", hostAgentIdFor("bound")],
      ])
      expect(dropped).toBe(6)

      const conflict = { tag: "conflict", val: "the key is current" } as const
      expect(
        yield* durable.client.create({ name: "unique" }, { keyed: true }).pipe(
          Effect.scoped,
          Effect.provide(
            rpcLayer({
              create: () => Effect.fail(new RpcHostError(conflict, "WasmRpc.create")),
            }),
          ),
          Effect.result,
        ),
      ).toMatchObject({ _tag: "Failure", failure: { _tag: "RpcCallError", cause: conflict } })
    }).pipe(Effect.provide(Layer.mergeAll(host, rpc)))
  })

  it.effect("validates reflected config before opening a scoped RPC connection", () => {
    const raw = registration()
    const valueType = raw.agentType.schema.typeNodes.length - 1
    raw.agentType.config = [
      { path: ["greeting"], source: "local", valueType },
      { path: ["apiKey"], source: "secret", valueType },
    ]
    const connections: ReadonlyArray<AgentCommon.TypedAgentConfigValue>[] = []
    const rpc = rpcLayer({
      create: (_name, _input, options) =>
        Effect.sync(() => {
          connections.push(options.agentConfig)
          return {
            connection: { drop: () => undefined } as RpcConnection,
            agentId: hostAgentIdFor("created"),
          }
        }),
    })
    const host = Layer.succeed(AgentHostClient, {
      getAgentType: () => raw,
    } as never)
    return Effect.gen(function* () {
      const reflected = (yield* getAgentType("Recursive"))!
      if (reflected.mode !== "durable") throw new Error("expected durable mode")
      expect(reflected.config[0]?.schema.graph.root).toBe(reflected.config[0]?.schema.root)
      for (const config of [
        [{ path: ["unknown"], value: "x" }],
        [{ path: ["apiKey"], value: "x" }],
        [{ path: ["greeting"], value: 42 }],
      ])
        expect(
          yield* Effect.scoped(
            reflected.client.create({ name: "one" }, { config }).pipe(Effect.result),
          ),
        ).toMatchObject({ _tag: "Failure" })
      expect(connections).toHaveLength(0)

      yield* Effect.scoped(
        reflected.client.create(
          { name: "one" },
          { config: [{ path: ["greeting"], value: "hello" }] },
        ),
      )
      yield* Effect.scoped(
        reflected.client.createValue(reflected.constructorInput.packJson({ name: "two" }), {
          keyed: true,
          config: [{ path: ["greeting"], value: reflected.config[0]!.schema.packJson("hello") }],
        }),
      )
      expect(connections).toHaveLength(2)
      expect(connections.map((entries) => entries.map((entry) => entry.path))).toEqual([
        [["greeting"]],
        [["greeting"]],
      ])
    }).pipe(Effect.provide(Layer.merge(host, rpc)))
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
        const invocationMetadata = hostInvocationMetadata("actual-id")
        const connection: RpcConnection = {
          agentId: () => Promise.resolve(hostAgentIdFor("actual-id")),
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
        const rpc = rpcLayer({
          upsert: () => Effect.succeed(connection),
          getById: () => Effect.succeed(connection),
        })
        const host = Layer.succeed(AgentHostClient, {
          getAgentType: () => registration(),
          getAgentTypeByAgentId: () => registration(),
        } as never)
        yield* Effect.scoped(
          Effect.gen(function* () {
            const type = (yield* getAgentType("Recursive"))!
            if (type.mode !== "durable") throw new Error("expected durable mode")
            const client = yield* type.client.upsert({ name: "unique" })
            const method = yield* client.method("walk")
            expect(Result.isFailure(yield* Effect.result(method.invoke({ node: {} })))).toBe(true)
            expect(events).toEqual([])
            expect(yield* method.invoke({ node: { label: "first", next: null } })).toEqual({
              metadata: sdkInvocationMetadata("actual-id"),
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
            const client = yield* bindDynamic(agentIdFor("opaque-id"))
            return yield* client.method("walk").invoke({ root: 0, valueNodes: [] })
          }),
        ).pipe(Effect.provide(Layer.mergeAll(host, rpc)), Effect.forkChild)
        while (!events.includes("get")) yield* Effect.yieldNow
        yield* Fiber.interrupt(fiber)
        expect(events).toEqual(["get", "cancel", "future-drop", "connection-drop"])
      }),
  )

  it.effect("attaches a per-call scope card to reflected awaited calls only", () =>
    Effect.gen(function* () {
      const scopeCards: Array<unknown> = []
      const raw = { scope: 1 } as unknown as CoreTypes.PermissionCard
      const scope = createScopeCard(raw)
      const invocationMetadata = hostInvocationMetadata("actual-id")
      const connection = {
        asyncInvokeAndAwait: (_name: string, _input: unknown, scopeCard: unknown) => {
          scopeCards.push(scopeCard)
          return {
            metadata: invocationMetadata,
            get: () =>
              Promise.resolve({ root: 0, valueNodes: [{ tag: "string-value", val: "answer" }] }),
            cancel: () => {},
            drop: () => {},
          }
        },
        invoke: () => invocationMetadata,
        drop: () => {},
      } as unknown as RpcConnection
      const rpc = rpcLayer({ upsert: () => Effect.succeed(connection) })
      const host = Layer.succeed(AgentHostClient, {
        getAgentType: () => registration(),
      } as never)
      yield* Effect.scoped(
        Effect.gen(function* () {
          const type = (yield* getAgentType("Recursive"))!
          if (type.mode !== "durable") throw new Error("expected durable mode")
          const client = yield* type.client.upsert({ name: "unique" })
          const method = yield* client.method("walk")
          const node = { node: { label: "first", next: null } }
          yield* method.invoke(node, { scopeCard: scope })
          yield* method.invoke(node)
          yield* method.trigger(node)
        }).pipe(Effect.provide(Layer.mergeAll(host, rpc))),
      )
      expect(scopeCards).toEqual([raw, undefined])
    }),
  )
})
