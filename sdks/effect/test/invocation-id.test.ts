import { describe, expect, it } from "@effect/vitest"
import { Effect, Result } from "effect"
import * as InvocationId from "../src/InvocationId.js"
import type { Uuid } from "../src/Ids.js"
import { invocationMetadataFromHost } from "../src/internal/rpc.js"
import { agentIdFor, hostInvocationMetadata } from "./invocation-metadata.js"

const uuid = "0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0" as Uuid
const agent = "00000001-0000-8000-8000-0000000000aa" as Uuid

describe("InvocationId", () => {
  it.effect("round-trips the `<agent-id>.<uuid>` text form", () =>
    Effect.gen(function* () {
      const id: InvocationId.InvocationId = { agentId: { uuid: agent }, uuid }
      const text = InvocationId.format(id)
      expect(text).toBe(`${agent}.${uuid}`)
      const parsed = yield* InvocationId.parse(text)
      expect(parsed).toEqual(id)
      expect(String(parsed)).toBe(text)
      expect(String(parsed.agentId)).toBe(agent)
      expect(Object.isFrozen(parsed)).toBe(true)
    }),
  )

  it.effect("accepts upper-case UUIDs and normalizes them to lower case", () =>
    Effect.gen(function* () {
      const parsed = yield* InvocationId.parse(`${agent.toUpperCase()}.${uuid.toUpperCase()}`)
      expect(parsed).toEqual({ agentId: { uuid: agent }, uuid })
      expect(parsed.toString()).toBe(`${agent}.${uuid}`)
    }),
  )

  it("formats host invocation metadata with toString", () => {
    const { agentId, invocationId } = invocationMetadataFromHost(hostInvocationMetadata("worker"))
    expect(agentId).toEqual(agentIdFor("worker"))
    expect(`${invocationId}`).toBe(
      `${agentIdFor("worker").uuid}.00000000-0000-0001-0000-000000000002`,
    )
    expect(String(invocationId)).toBe(InvocationId.format(invocationId))
  })

  it.effect.each([
    ["a missing dot", `${agent}${uuid}`],
    ["a readable agent ID", `Worker().${uuid}`],
    ["an invalid UUID", `${agent}.0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1fz`],
    ["a non-hyphenated UUID", `${agent}.${uuid.replaceAll("-", "")}`],
    ["misplaced hyphens", `${agent}.0f1e2d3c4-b5a-4968-8776-a5b4c3d2e1f0`],
    ["an empty agent ID", `.${uuid}`],
    ["a second dot", `${agent}.${uuid}.x`],
    ["an empty text", ""],
  ])("rejects %s", ([, text]) =>
    Effect.gen(function* () {
      const result = yield* Effect.result(InvocationId.parse(text))
      expect(Result.isFailure(result)).toBe(true)
      if (Result.isFailure(result)) {
        expect(result.failure._tag).toBe("InvocationIdParseError")
        expect(result.failure.input).toBe(text)
      }
    }),
  )
})
