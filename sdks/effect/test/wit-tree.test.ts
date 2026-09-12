import { describe, expect, it } from "vitest"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { schemaGraphFromWit, schemaValueFromWit } from "../src/internal/witTree.js"

describe("schema graph/value tree boundaries", () => {
  it("rejects an out-of-range schema root", () => {
    expect(() => schemaGraphFromWit({ typeNodes: [], defs: [], root: 0 })).toThrow(
      /root.*out of range|index out of range/,
    )
  })

  it("rejects trailing unreachable value nodes", () => {
    const malformed: CoreTypes.SchemaValueTree = {
      valueNodes: [
        { tag: "bool-value", val: true },
        { tag: "string-value", val: "trailing" },
      ],
      root: 0,
    }
    expect(() => schemaValueFromWit(malformed)).toThrow(/unreachable|trailing/)
  })
})
