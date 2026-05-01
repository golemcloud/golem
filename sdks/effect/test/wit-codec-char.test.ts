import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/WitCodec.js"
import { Char } from "../src/WitTypes.js"

const roundtrip = (s: Schema.Top, v: unknown) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(s as any)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    const wv = yield* Schema.encodeEffect(codec)(v)
    const back = yield* Schema.decodeEffect(codec)(wv)
    return { wc, wv, back }
  })

describe("Char → WIT prim-char", () => {
  it.effect("emits prim-char-type and round-trips a single character", () =>
    Effect.gen(function* () {
      const r = yield* roundtrip(Char, "x")
      expect(r.wc.witType.nodes[0]?.type.tag).toBe("prim-char-type")
      expect(r.wv.nodes[0]?.tag).toBe("prim-char")
      expect(r.back).toBe("x")
    }),
  )

  it.effect("Schema.String stays prim-string-type", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Schema.String)
      expect(wc.witType.nodes[0]?.type.tag).toBe("prim-string-type")
    }),
  )

  it.effect("rejects multi-character input via Schema.Char's length check", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Char)
      const codec = wc.codec as Schema.Codec<any, any, never, never>
      const exit = yield* Effect.exit(Schema.encodeEffect(codec)("abc"))
      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("Char nested in a struct round-trips", () =>
    Effect.gen(function* () {
      const S = Schema.Struct({ initial: Char, name: Schema.String })
      const r = yield* roundtrip(S, { initial: "A", name: "Ada" })
      expect(r.back).toEqual({ initial: "A", name: "Ada" })
      const recordNode = r.wc.witType.nodes[0]?.type as any
      expect(recordNode.tag).toBe("record-type")
      const initialIdx = recordNode.val.find((p: [string, number]) => p[0] === "initial")[1]
      expect(r.wc.witType.nodes[initialIdx]?.type.tag).toBe("prim-char-type")
    }),
  )
})
