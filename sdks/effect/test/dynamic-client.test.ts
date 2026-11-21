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
  hostInvocationMetadata,
  sdkInvocationMetadata,
} from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

const tree: CoreTypes.SchemaValueTree = { root: 0, valueNodes: [] }
const metadata = hostInvocationMetadata("agent")

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

  it.effect("makes throwing scheduled cancellation best-effort and idempotent, then drops", () =>
    Effect.gen(function* () {
      let cancels = 0
      let drops = 0
      const connection = {
        invoke: () => metadata,
        scheduleCancelableInvocation: () => ({
          metadata,
          token: {
            cancel: () => {
              cancels++
              throw new Error("host cancel failed")
            },
            drop: () => {
              drops++
            },
          },
        }),
        drop: () => undefined,
      } as unknown as RpcConnection
      const rpc = rpcLayer({ getById: () => Effect.succeed(connection) })

      yield* Effect.scoped(
        Effect.gen(function* () {
          const client = yield* bind(agentIdFor("agent"))
          const triggered = yield* client.method("run").trigger(tree)
          expect(triggered).toEqual(sdkInvocationMetadata("agent"))
          expect(triggered).not.toBe(metadata)
          expect(Object.isFrozen(triggered)).toBe(true)
          const scheduled = yield* client
            .method("run")
            .schedule({ seconds: 1n, nanoseconds: 0 }, tree)
          expect(scheduled.metadata).toEqual(sdkInvocationMetadata("agent"))
          expect(scheduled.metadata).not.toBe(metadata)
          expect(Object.isFrozen(scheduled.metadata)).toBe(true)
          yield* scheduled.cancel
          yield* scheduled.cancel
        }),
      ).pipe(Effect.provide(Layer.merge(hostLayer(), rpc)))

      expect(cancels).toBe(1)
      expect(drops).toBe(1)
    }),
  )
})
