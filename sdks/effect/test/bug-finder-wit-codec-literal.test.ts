import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { compile } from "../src/WitCodec.js"
import type { SchemaValueNode } from "golem:core/types@2.0.0"

describe("WitCodec literal wire validation", () => {
  it.effect("rejects a wire value different from the declared literal", () =>
    Effect.gen(function* () {
      const codec = yield* compile(Schema.Literal("expected"))
      const exit = yield* Effect.exit(
        codec.decode({
          valueNodes: [{ tag: "string-value", val: "different" }],
          root: 0,
        }),
      )

      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("validates boolean, numeric, and bigint literal payloads", () =>
    Effect.gen(function* () {
      const cases: ReadonlyArray<readonly [Schema.Codec<any, any>, SchemaValueNode]> = [
        [Schema.Literal(true), { tag: "bool-value", val: false }],
        [Schema.Literal(17), { tag: "f64-value", val: 18 }],
        [Schema.Literal(23n), { tag: "s64-value", val: 24n }],
      ]
      for (const [schema, node] of cases) {
        const codec = yield* compile(schema)
        const exit = yield* Effect.exit(codec.decode({ valueNodes: [node], root: 0 }))
        expect(exit._tag).toBe("Failure")
      }
    }),
  )
})
