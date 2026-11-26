// Every reflected `trigger` and `schedule` entry point rejects a method whose input or output
// contains a live stream before any host call, as the definition client does.
import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import type { RpcConnection } from "../src/host/RpcClient.js"
import { getAgentType } from "../src/Reflection.js"
import { emptyMetadata, field, t, type SchemaGraph } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"
import { fakeRpcInvocation } from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

const metadata = emptyMetadata()

// Node layout: root = record { name: string }, then string, then stream<string>.
const registration = (): AgentHost.RegisteredAgentType => {
  const graph: SchemaGraph = { defs: new Map(), root: t.record([field("name", t.string())]) }
  const schema = schemaGraphToWit(graph)
  const stringIndex = schema.typeNodes.length
  schema.typeNodes.push({ body: { tag: "string-type" }, metadata })
  const streamIndex = schema.typeNodes.length
  schema.typeNodes.push({ body: { tag: "stream-type", val: stringIndex }, metadata })
  return {
    agentType: {
      typeName: "Streamer",
      kind: "regular",
      description: "reflected agent with streaming methods",
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
          name: "watch",
          description: "returns a live stream",
          httpEndpoint: [],
          inputSchema: { tag: "parameters", val: [] },
          outputSchema: { tag: "single", val: streamIndex },
        },
        {
          name: "feed",
          description: "takes a live stream",
          httpEndpoint: [],
          inputSchema: {
            tag: "parameters",
            val: [
              { name: "lines", source: { tag: "user-supplied" }, schema: streamIndex, metadata },
            ],
          },
          outputSchema: { tag: "unit" },
        },
      ],
      dependencies: [],
      snapshotting: { tag: "disabled" },
      config: [],
    },
  } as unknown as AgentHost.RegisteredAgentType
}

describe("reflected trigger/schedule of a streaming method", () => {
  it.effect.each(["watch", "feed"])("reject %s before calling the host", (name) =>
    Effect.gen(function* () {
      const hostCalls: string[] = []
      const connection = {
        invokeAndAwait: () => Promise.reject(new Error("unexpected awaited call")),
        trigger: async () => {
          hostCalls.push("trigger")
          return fakeRpcInvocation("actual-id")
        },
        schedule: async () => {
          hostCalls.push("schedule")
          return fakeRpcInvocation("actual-id")
        },
        drop: () => {},
      } as unknown as RpcConnection
      const rpc = rpcLayer({ upsert: () => Effect.succeed(connection) })
      const host = Layer.succeed(AgentHostClient, {
        getAgentType: () => registration(),
      } as never)
      const at = { seconds: 1n, nanoseconds: 0 }
      const tree = {} as never
      const results = yield* Effect.scoped(
        Effect.gen(function* () {
          const type = (yield* getAgentType("Streamer"))!
          if (type.mode !== "durable") throw new Error("expected durable mode")
          const client = yield* type.client.upsert({ name: "one" })
          const method = yield* client.method(name)
          return [
            yield* Effect.flip(method.trigger({})),
            yield* Effect.flip(method.triggerValue(tree)),
            yield* Effect.flip(method.schedule(at, {})),
            yield* Effect.flip(method.scheduleValue(at, tree)),
          ]
        }).pipe(Effect.provide(Layer.mergeAll(host, rpc))),
      )
      expect(hostCalls).toEqual([])
      expect(results).toEqual(
        Array(4).fill({
          _tag: "RemoteResponseError",
          reason: `${name}: live streams require invoke-and-await`,
        }),
      )
    }),
  )
})
