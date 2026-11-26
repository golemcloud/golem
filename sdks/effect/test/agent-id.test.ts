import { describe, expect, it } from "@effect/vitest"
import { Effect } from "effect"
import { uuidToString } from "golem:core/types@2.0.0"
import * as AgentId from "../src/AgentId.js"
import { agentIdFromWit, agentIdToWit } from "../src/internal/agentId.js"
import * as Package from "../src/index.js"
import { hostAgentIdFor } from "./invocation-metadata.js"

describe("AgentId", () => {
  it.effect("parses the UUID text form to the lower-case text", () =>
    Effect.gen(function* () {
      const text = uuidToString(hostAgentIdFor("parsed").uuid)
      const id = yield* AgentId.parse(text.toUpperCase())
      expect(id).toBe(text)
      expect(agentIdToWit(id)).toEqual(hostAgentIdFor("parsed"))
      expect(agentIdFromWit(hostAgentIdFor("parsed"))).toBe(id)
      expect(Package.AgentId.parse).toBe(AgentId.parse)
    }),
  )

  it.effect("rejects texts that are not UUIDs", () =>
    Effect.gen(function* () {
      for (const text of ["", "Counter(1)", "not-a-uuid", `${"0".repeat(32)}`]) {
        const error = yield* Effect.flip(AgentId.parse(text))
        expect(error).toBeInstanceOf(AgentId.AgentIdParseError)
        expect(error).toMatchObject({ _tag: "AgentIdParseError", input: text })
      }
    }),
  )
})
