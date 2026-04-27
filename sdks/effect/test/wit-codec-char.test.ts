import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/wit-codec.js"
import { Char } from "../src/wit-types.js"

const compile = (s: Schema.Top) => Effect.runPromise(toWitCodec(s as any))

const roundtrip = async (s: Schema.Top, v: unknown) => {
  const wc = await compile(s)
  const codec = wc.codec as Schema.Codec<any, any, never, never>
  const wv = await Effect.runPromise(Schema.encodeEffect(codec)(v))
  const back = await Effect.runPromise(Schema.decodeEffect(codec)(wv))
  return { wc, wv, back }
}

describe("Char → WIT prim-char", () => {
  it("emits prim-char-type and round-trips a single character", async () => {
    const r = await roundtrip(Char, "x")
    expect(r.wc.witType.nodes[0]?.type.tag).toBe("prim-char-type")
    expect(r.wv.nodes[0]?.tag).toBe("prim-char")
    expect(r.back).toBe("x")
  })

  it("Schema.String stays prim-string-type", async () => {
    const wc = await compile(Schema.String)
    expect(wc.witType.nodes[0]?.type.tag).toBe("prim-string-type")
  })

  it("rejects multi-character input via Schema.Char's length check", async () => {
    const wc = await compile(Char)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    await expect(Effect.runPromise(Schema.encodeEffect(codec)("abc"))).rejects.toThrow()
  })

  it("Char nested in a struct round-trips", async () => {
    const S = Schema.Struct({ initial: Char, name: Schema.String })
    const r = await roundtrip(S, { initial: "A", name: "Ada" })
    expect(r.back).toEqual({ initial: "A", name: "Ada" })
    // Confirm the nested element really is prim-char.
    const recordNode = r.wc.witType.nodes[0]?.type as any
    expect(recordNode.tag).toBe("record-type")
    const initialIdx = recordNode.val.find((p: [string, number]) => p[0] === "initial")[1]
    expect(r.wc.witType.nodes[initialIdx]?.type.tag).toBe("prim-char-type")
  })
})
