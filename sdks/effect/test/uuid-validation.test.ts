import { describe, expect, it } from "vitest"
import { t, v } from "../src/internal/schema-model/model.js"
import { schemaValueMatches } from "../src/internal/reflection/schemaValidation.js"

describe("UUID schema value validation", () => {
  it("rejects UUID halves outside the unsigned 64-bit range", () => {
    const graph = { root: t.uuid(), defs: new Map() }

    expect(schemaValueMatches(graph, graph.root, v.uuid({ highBits: -1n, lowBits: 0n }))).toBe(
      false,
    )
    expect(
      schemaValueMatches(graph, graph.root, v.uuid({ highBits: 0n, lowBits: 1n << 64n })),
    ).toBe(false)
  })
})
