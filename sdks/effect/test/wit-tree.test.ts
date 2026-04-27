import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import type * as CoreTypes from "golem:core/types@1.5.0"
import { witGraphCodec, type WitValueTree } from "../src/wit-tree.js"

/**
 * WitType describing `record { name: string, alive: bool, scores: list<u32> }`,
 * hand-built so the indices are guaranteed correct (sidesteps anything we
 * may need to fix in the AST→WitType converter).
 *
 * Layout:
 *   0: record { name -> 1, alive -> 2, scores -> 3 }
 *   1: prim-string
 *   2: prim-bool
 *   3: list-type val:4
 *   4: prim-u32
 */
const recordType: CoreTypes.WitType = {
  nodes: [
    {
      name: "Thing",
      type: {
        tag: "record-type",
        val: [
          ["name", 1],
          ["alive", 2],
          ["scores", 3],
        ],
      },
    },
    { type: { tag: "prim-string-type" } },
    { type: { tag: "prim-bool-type" } },
    { type: { tag: "list-type", val: 4 } },
    { type: { tag: "prim-u32-type" } },
  ],
}

/**
 * Matching graph value: `{ name: "Ada", alive: true, scores: [1, 2, 3] }`.
 *
 *   0: record-value [1, 2, 3]
 *   1: prim-string "Ada"
 *   2: prim-bool true
 *   3: list-value [4, 5, 6]
 *   4: prim-u32 1
 *   5: prim-u32 2
 *   6: prim-u32 3
 */
const recordValue: CoreTypes.WitValue = {
  nodes: [
    { tag: "record-value", val: [1, 2, 3] },
    { tag: "prim-string", val: "Ada" },
    { tag: "prim-bool", val: true },
    { tag: "list-value", val: [4, 5, 6] },
    { tag: "prim-u32", val: 1 },
    { tag: "prim-u32", val: 2 },
    { tag: "prim-u32", val: 3 },
  ],
}

const expectedTree: WitValueTree = {
  tag: "record-value",
  val: [
    { tag: "prim-string", val: "Ada" },
    { tag: "prim-bool", val: true },
    {
      tag: "list-value",
      val: [
        { tag: "prim-u32", val: 1 },
        { tag: "prim-u32", val: 2 },
        { tag: "prim-u32", val: 3 },
      ],
    },
  ],
}

describe("witGraphCodec", () => {
  it("inflates a WitValue graph into a WitValueTree", async () => {
    const codec = witGraphCodec(recordType)
    const tree = await Effect.runPromise(Schema.decodeEffect(codec)(recordValue))
    expect(tree).toEqual(expectedTree)
  })

  it("flattens a WitValueTree back into a structurally-equivalent WitValue", async () => {
    const codec = witGraphCodec(recordType)
    const graph = await Effect.runPromise(Schema.encodeEffect(codec)(expectedTree))

    // Round-trip back to a tree to validate semantic equivalence (the
    // concrete index assignment may differ because we always emit
    // root-first depth-first).
    const tree = await Effect.runPromise(Schema.decodeEffect(codec)(graph))
    expect(tree).toEqual(expectedTree)
  })

  it("handles option, variant, tuple, and result", async () => {
    /**
     * Type:
     *   0: variant Outcome { ok -> 1, err -> 4 }
     *   1: tuple [2, 3]
     *   2: option-type val:5  (option<string>)
     *   3: prim-bool
     *   4: prim-string
     *   5: prim-string
     */
    const t: CoreTypes.WitType = {
      nodes: [
        {
          name: "Outcome",
          type: {
            tag: "variant-type",
            val: [
              ["ok", 1],
              ["err", 4],
            ],
          },
        },
        { type: { tag: "tuple-type", val: [2, 3] } },
        { type: { tag: "option-type", val: 5 } },
        { type: { tag: "prim-bool-type" } },
        { type: { tag: "prim-string-type" } },
        { type: { tag: "prim-string-type" } },
      ],
    }
    /** value: ok((Some("hi"), false)) */
    const v: CoreTypes.WitValue = {
      nodes: [
        { tag: "variant-value", val: [0, 1] },
        { tag: "tuple-value", val: [2, 3] },
        { tag: "option-value", val: 4 },
        { tag: "prim-bool", val: false },
        { tag: "prim-string", val: "hi" },
      ],
    }
    const codec = witGraphCodec(t)
    const tree = await Effect.runPromise(Schema.decodeEffect(codec)(v))
    expect(tree).toEqual({
      tag: "variant-value",
      val: [
        0,
        {
          tag: "tuple-value",
          val: [
            { tag: "option-value", val: { tag: "prim-string", val: "hi" } },
            { tag: "prim-bool", val: false },
          ],
        },
      ],
    })

    // Round-trip.
    const g2 = await Effect.runPromise(Schema.encodeEffect(codec)(tree))
    const tree2 = await Effect.runPromise(Schema.decodeEffect(codec)(g2))
    expect(tree2).toEqual(tree)
  })
})
