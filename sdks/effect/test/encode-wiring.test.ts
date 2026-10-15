import { describe, expect, it } from "@effect/vitest"
import { vi } from "vitest"
import { Effect, Fiber, Layer, Schema } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as AgentIdentity from "../src/AgentIdentity.js"
import { defineAgent } from "../src/Agent.js"
import * as Bridge from "../src/Bridge.js"
import * as DynamicClient from "../src/DynamicClient.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { PermissionsClient, type PermissionsClientShape } from "../src/host/PermissionsClient.js"
import { RpcClient, type RpcConnection } from "../src/host/RpcClient.js"
import * as rpc from "../src/internal/rpc.js"
import { method } from "../src/Method.js"
import * as Permissions from "../src/Permissions.js"
import * as WitTypes from "../src/WitTypes.js"

// Runs right after an SDK wire encode has moved the capabilities and before its promise
// resolves, which is where an interruption used to lose them.
const gate = vi.hoisted(() => ({ afterEncode: undefined as (() => void) | undefined }))
const fireAfterEncode = () => {
  const hook = gate.afterEncode
  gate.afterEncode = undefined
  hook?.()
}

vi.mock("../src/internal/schema-model/wit.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/internal/schema-model/wit.js")>()
  return {
    ...actual,
    // The asynchronous encode of `Bridge` and of schema codecs without a direct wire codec. Direct
    // wire codecs never carry capabilities, so they need no hook.
    schemaValueToWitAsync: async (
      ...args: Parameters<typeof actual.schemaValueToWitAsync>
    ): ReturnType<typeof actual.schemaValueToWitAsync> => {
      const tree = await actual.schemaValueToWitAsync(...args)
      fireAfterEncode()
      return tree
    },
  }
})

vi.mock("../src/internal/rpc.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/internal/rpc.js")>()
  return { ...actual, awaitInvocation: vi.fn(actual.awaitInvocation) }
})

const Reader = defineAgent({
  name: "WiringReader",
  id: { name: Schema.String },
  methods: {
    give: method({
      input: { card: WitTypes.PermissionCard({ polymorphic: false }) },
      success: Schema.Void,
    }),
  },
})

const layers = (calls: Array<string>) => {
  const metadata: AgentHost.InvocationMetadata = { agentId: "reader", idempotencyKey: "key" }
  let serial = 0
  const permissions: Partial<PermissionsClientShape> = {
    deriveFromWallet: () => ({ serial: ++serial }) as unknown as CoreTypes.PermissionCard,
    inspect: () => ({ lowerPositive: [], lowerNegative: [], upperPositive: [], upperNegative: [] }),
  }
  const connection = {
    asyncInvokeAndAwait: (name: string) => {
      calls.push(name)
      return { metadata, get: () => Promise.resolve(undefined), cancel: () => {}, drop: () => {} }
    },
    drop: () => {},
  } as unknown as RpcConnection
  return Layer.mergeAll(
    Layer.succeed(PermissionsClient, PermissionsClient.of(permissions as PermissionsClientShape)),
    Layer.succeed(RpcClient, RpcClient.of({ connect: () => Effect.succeed(connection) })),
    Layer.succeed(
      AgentHostClient,
      AgentHostClient.of({
        makeAgentId: () => "reader",
        parseAgentId: () => ["WiringReader", { value: { root: 0, valueNodes: [] } }, undefined],
      } as never),
    ),
  )
}

// Interrupt `call` from inside its encode, after the capabilities moved into the wire tree.
const interruptAfterEncode = <A, E, R>(call: Effect.Effect<A, E, R>) =>
  Effect.gen(function* () {
    let fired = false
    gate.afterEncode = () => {
      fired = true
      Effect.runFork(Fiber.interrupt(fiber))
    }
    const fiber = yield* Effect.forkChild(call)
    yield* Fiber.await(fiber)
    gate.afterEncode = undefined
    yield* Effect.promise(() => new Promise((resolve) => setTimeout(resolve, 0)))
    expect(fired).toBe(true)
  })

describe("wire encode wiring", () => {
  it.effect("typed clients give encoded cards back when interrupted after encoding", () => {
    const calls: Array<string> = []
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const reader = yield* Reader.client.get({ name: "a" })
      yield* interruptAfterEncode(reader.give({ card }))
      expect(calls).toEqual([])
      yield* Permissions.inspect(card)
    }).pipe(Effect.scoped, Effect.provide(layers(calls)))
  })

  it.effect("bridge handles give encoded cards back when interrupted after encoding", () => {
    const calls: Array<string> = []
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const handle = yield* Bridge.resolveRemoteAgent(
        "WiringReader",
        { tag: "record", fields: [] },
        undefined,
        [],
        "durable",
      )
      const input: Bridge.SchemaValue = {
        tag: "record",
        fields: [Bridge.permissionCardToSchemaValue(card)],
      }
      yield* interruptAfterEncode(handle.invokeAndAwait("give", input, () => undefined))
      expect(calls).toEqual([])
      yield* Permissions.inspect(card)
    }).pipe(Effect.scoped, Effect.provide(layers(calls)))
  })

  it.effect("dynamic calls never restore the caller's tree", () => {
    const calls: Array<string> = []
    return Effect.gen(function* () {
      const identity = yield* AgentIdentity.parse('WiringReader("a")')
      const dynamic = yield* DynamicClient.bind(identity)
      vi.mocked(rpc.awaitInvocation).mockClear()
      yield* dynamic.method("give").invoke({ root: 0, valueNodes: [] })
      expect(vi.mocked(rpc.awaitInvocation).mock.calls.map((args) => args[4])).toEqual([false])
      expect(calls).toEqual(["give"])
    }).pipe(Effect.scoped, Effect.provide(layers(calls)))
  })
})
