import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer } from "effect"
import { vi } from "vitest"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as AgentIdentity from "../src/AgentIdentity.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcClient, type RpcClientShape, type RpcConnection } from "../src/host/RpcClient.js"
import { decodeAgentId } from "../src/internal/agentIdentity.js"

describe("identity boundaries", () => {
  it("keeps raw decoding unwrapped, uncopied, and uncached", () => {
    const typed: CoreTypes.TypedSchemaValue = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [], root: 0 },
    }
    const tuple: ReturnType<typeof decodeAgentId> = ["Agent", typed, undefined]
    const parseAgentId = vi.fn(() => tuple)
    expect(decodeAgentId({ parseAgentId }, "encoded")).toBe(tuple)
    expect(decodeAgentId({ parseAgentId }, "encoded")[1]).toBe(typed)
    expect(parseAgentId).toHaveBeenCalledTimes(2)
    const cause = { tag: "invalid-input", val: "invalid ID" }
    try {
      decodeAgentId(
        {
          parseAgentId: () => {
            throw cause
          },
        },
        "invalid",
      )
      expect.fail("expected host error")
    } catch (error) {
      expect(error).toBe(cause)
    }
  })

  it.effect("preserves opaque constructor handles and raw phantom identity when binding", () =>
    Effect.gen(function* () {
      class OpaqueCard {}
      const card = new OpaqueCard() as CoreTypes.PermissionCard
      const tree: CoreTypes.SchemaValueTree = {
        root: 0,
        valueNodes: [{ tag: "permission-card-handle", val: card }],
      }
      const phantom = { highBits: 1n, lowBits: 2n }
      const parseAgentId = vi.fn(
        (): ReturnType<typeof decodeAgentId> => [
          "Agent",
          { graph: { typeNodes: [], defs: [], root: 0 }, value: tree },
          phantom,
        ],
      )
      const drop = vi.fn()
      const connect = vi.fn<RpcClientShape["connect"]>(() =>
        Effect.succeed({ drop } as unknown as RpcConnection),
      )
      const identity = yield* AgentIdentity.parse("encoded").pipe(
        Effect.provideService(AgentHostClient, { parseAgentId } as never),
      )
      expect(Object.isFrozen(identity)).toBe(true)
      expect(Object.isFrozen(card)).toBe(false)
      expect(identity.constructorValue).not.toBe(tree)
      expect(identity.constructorValue.valueNodes[0]).toEqual(tree.valueNodes[0])
      yield* identity
        .dynamicClient()
        .pipe(Effect.scoped, Effect.provide(Layer.succeed(RpcClient, { connect })))
      expect(parseAgentId).toHaveBeenCalledTimes(1)
      expect(connect.mock.calls[0]?.[2]).toBe(phantom)
      expect(drop).toHaveBeenCalledTimes(1)
    }),
  )
})
