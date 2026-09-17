import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/WitCodec.js"
import { Char } from "../src/WitTypes.js"

const roundtrip = (s: Schema.Top, v: unknown) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(s as any)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    const sv = yield* Schema.encodeEffect(codec)(v)
    const back = yield* Schema.decodeEffect(codec)(sv)
    return { wc, sv, back }
  })

describe("Char → schema char", () => {
  it.effect("emits a char node and round-trips a single character", () =>
    Effect.gen(function* () {
      const r = yield* roundtrip(Char, "x")
      expect(r.wc.graph.root.body.tag).toBe("char")
      expect((r.sv as any).tag).toBe("char")
      expect(r.back).toBe("x")
    }),
  )

  it.effect("Schema.String stays a string node", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Schema.String)
      expect(wc.graph.root.body.tag).toBe("string")
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
      const recordBody = r.wc.graph.root.body as any
      expect(recordBody.tag).toBe("record")
      const initialField = recordBody.fields.find((f: any) => f.name === "initial")
      expect(initialField.body.body.tag).toBe("char")
    }),
  )
})
