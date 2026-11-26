import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { bind } from "../src/DynamicClient.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"
import {
  agentIdFor,
  hostAgentIdFor,
  fakeRpcInvocation,
  sdkInvocationMetadata,
} from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

const tree: CoreTypes.SchemaValueTree = { root: 0, valueNodes: [] }

const hostLayer = (mode: AgentCommon.AgentMode | "missing" = "durable") =>
  Layer.succeed(AgentHostClient, {
    getAgentTypeByAgentId: () =>
      mode === "missing" ? undefined : { agentType: { typeName: "Agent", mode } },
  } as never)

describe("DynamicClient", () => {
  it.effect("binds by agent ID and reports unknown IDs, ephemeral types and host errors", () =>
    Effect.gen(function* () {
      const opened: Array<readonly [string, CoreTypes.AgentId]> = []
      const connection = {
        agentId: () => Promise.resolve(hostAgentIdFor("dynamic")),
        drop: () => undefined,
      } as unknown as RpcConnection
      const rpc = rpcLayer({
        getById: (agentType, id) => {
          opened.push([agentType, id])
          return Effect.succeed(connection)
        },
      })
      const client = yield* bind(agentIdFor("dynamic")).pipe(
        Effect.scoped,
        Effect.provide(Layer.merge(hostLayer(), rpc)),
      )
      expect(client.agentId).toEqual(agentIdFor("dynamic"))
      expect(opened).toEqual([["Agent", hostAgentIdFor("dynamic")]])

      for (const mode of ["missing", "ephemeral"] as const) {
        const result = yield* bind(agentIdFor("dynamic")).pipe(
          Effect.scoped,
          Effect.provide(Layer.merge(hostLayer(mode), rpc)),
          Effect.result,
        )
        expect(result).toMatchObject({
          _tag: "Failure",
          failure:
            mode === "missing"
              ? { _tag: "RpcCallError", cause: { tag: "not-found" } }
              : { _tag: "ClientBindingError" },
        })
      }
      expect(opened).toHaveLength(1)

      const structured = {
        tag: "remote-agent-error" as const,
        val: { tag: "invalid", val: { value: tree } },
      }
      const failing = rpcLayer({
        getById: () => Effect.fail(new RpcHostError(structured, "WasmRpc.getById")),
      })
      expect(
        yield* bind(agentIdFor("dynamic")).pipe(
          Effect.scoped,
          Effect.provide(Layer.merge(hostLayer(), failing)),
          Effect.result,
        ),
      ).toMatchObject({ _tag: "Failure", failure: { _tag: "RpcCallError", cause: structured } })
    }),
  )

  it.effect("returns scoped handles from trigger and schedule and maps cancel failures", () =>
    Effect.gen(function* () {
      const events: string[] = []
      const scheduledHandle = {
        ...fakeRpcInvocation("agent", undefined, events),
        cancel: () => Promise.reject({ tag: "cannot-cancel-running", val: "running" }),
      }
      const connection = {
        trigger: async () => fakeRpcInvocation("agent", tree, events),
        schedule: async () => scheduledHandle,
        drop: () => undefined,
      } as unknown as RpcConnection
      const rpc = rpcLayer({ getById: () => Effect.succeed(connection) })

      yield* Effect.scoped(
        Effect.gen(function* () {
          const client = yield* bind(agentIdFor("agent"))
          const triggered = yield* client.method("run").trigger(tree)
          expect(triggered.id).toEqual(sdkInvocationMetadata("agent").invocationId)
          expect(Object.isFrozen(triggered)).toBe(true)
          expect(yield* triggered.awaitResult).toBe(tree)
          expect(yield* triggered.cancel).toBe(true)
          const scheduled = yield* client
            .method("run")
            .schedule({ seconds: 1n, nanoseconds: 0 }, tree)
          expect(scheduled.id).toEqual(sdkInvocationMetadata("agent").invocationId)
          expect(yield* Effect.flip(scheduled.cancel)).toEqual({
            _tag: "RpcCannotCancelRunning",
            reason: "running",
          })
          expect(events).toEqual(["handle-cancel"])
        }),
      ).pipe(Effect.provide(Layer.merge(hostLayer(), rpc)))

      expect(events).toEqual(["handle-cancel", "handle-drop", "handle-drop"])
    }),
  )
})
