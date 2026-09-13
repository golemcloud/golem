import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { fromAgentId } from "../src/DynamicClient.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcClient, RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"

const tree: CoreTypes.SchemaValueTree = { root: 0, valueNodes: [] }
const metadata = { agentId: "agent", idempotencyKey: "key" }

const hostLayer = (parseAgentId: () => unknown = () => ["Agent", { value: tree }, undefined]) =>
  Layer.succeed(AgentHostClient, { parseAgentId } as never)

describe("DynamicClient", () => {
  it.effect("distinguishes identity parsing from remote call errors", () =>
    Effect.gen(function* () {
      const structured = {
        tag: "remote-agent-error" as const,
        val: { tag: "invalid", val: { value: tree } },
      }
      const identityError = { tag: "invalid-input", val: "malformed agent ID" }
      const unusedRpc = Layer.succeed(RpcClient, {
        connect: () => Effect.die("parse failure must not connect"),
      })
      const parse = yield* fromAgentId("bad").pipe(
        Effect.provide(
          Layer.merge(
            hostLayer(() => {
              throw identityError
            }),
            unusedRpc,
          ),
        ),
        Effect.result,
      )
      expect(parse).toMatchObject({
        _tag: "Failure",
        failure: { _tag: "AgentIdentityError", cause: identityError },
      })

      const rpc = Layer.succeed(RpcClient, {
        connect: () => Effect.fail(new RpcHostError(structured, "WasmRpc.create")),
      })
      const connect = yield* fromAgentId("valid").pipe(
        Effect.scoped,
        Effect.provide(Layer.merge(hostLayer(), rpc)),
        Effect.result,
      )
      expect(connect).toMatchObject({
        _tag: "Failure",
        failure: { _tag: "RpcCallError", cause: structured },
      })
    }),
  )

  it.effect("makes throwing scheduled cancellation best-effort and idempotent, then drops", () =>
    Effect.gen(function* () {
      let cancels = 0
      let drops = 0
      const connection = {
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
      const rpc = Layer.succeed(RpcClient, { connect: () => Effect.succeed(connection) })

      yield* Effect.scoped(
        Effect.gen(function* () {
          const client = yield* fromAgentId("valid")
          const scheduled = yield* client
            .method("run")
            .schedule({ seconds: 1n, nanoseconds: 0 }, tree)
          yield* scheduled.cancel
          yield* scheduled.cancel
        }),
      ).pipe(Effect.provide(Layer.merge(hostLayer(), rpc)))

      expect(cancels).toBe(1)
      expect(drops).toBe(1)
    }),
  )
})
