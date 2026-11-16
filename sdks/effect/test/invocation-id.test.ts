import { describe, expect, it } from "@effect/vitest"
import { Effect, Result } from "effect"
import * as InvocationId from "../src/InvocationId.js"
import type { Uuid } from "../src/Ids.js"
import { invocationMetadataFromHost } from "../src/internal/rpc.js"
import { hostInvocationMetadata } from "./invocation-metadata.js"

const uuid = "0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0" as Uuid

describe("InvocationId", () => {
  it.effect("round-trips an agent ID that contains '.', '/' and quotes", () =>
    Effect.gen(function* () {
      const id: InvocationId.InvocationId = {
        agentId: `Worker("a.b/c", 'd.e', "x\\"y").v2`,
        uuid,
      }
      const text = InvocationId.format(id)
      expect(text).toBe(`Worker("a.b/c", 'd.e', "x\\"y").v2.${uuid}`)
      const parsed = yield* InvocationId.parse(text)
      expect(parsed).toEqual(id)
      expect(String(parsed)).toBe(text)
      expect(Object.isFrozen(parsed)).toBe(true)
    }),
  )

  it.effect("accepts an upper-case UUID and normalizes it to lower case", () =>
    Effect.gen(function* () {
      const parsed = yield* InvocationId.parse(`Worker().${uuid.toUpperCase()}`)
      expect(parsed).toEqual({ agentId: "Worker()", uuid })
      expect(parsed.toString()).toBe(`Worker().${uuid}`)
    }),
  )

  it("formats host invocation metadata with toString", () => {
    const { invocationId } = invocationMetadataFromHost(hostInvocationMetadata('Worker("a.b")'))
    expect(`${invocationId}`).toBe(`Worker("a.b").00000000-0000-0001-0000-000000000002`)
    expect(String(invocationId)).toBe(InvocationId.format(invocationId))
  })

  it.effect.each([
    ["a missing dot", `Worker()${uuid}x`],
    ["a text shorter than the UUID suffix", "Worker().1234"],
    ["an invalid UUID", "Worker().0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1fz"],
    ["a non-hyphenated UUID", `Worker().${uuid.replaceAll("-", "")}`],
    ["misplaced hyphens", "Worker().0f1e2d3c4-b5a-4968-8776-a5b4c3d2e1f0"],
    ["an empty agent ID", `.${uuid}`],
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
