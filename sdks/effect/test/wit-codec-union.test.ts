import { describe, it, expect } from "vitest"
import { Effect, Result, Schema } from "effect"
import { toWitCodec, UnsupportedSchemaError } from "../src/wit-codec.js"
import { Int32, withVariantCaseName } from "../src/wit-types.js"

const compile = (s: Schema.Top) => Effect.runPromise(toWitCodec(s as any))

const tryCompile = (s: Schema.Top) =>
  Effect.runPromise(Effect.result(toWitCodec(s as any))) as Promise<
    Result.Result<unknown, UnsupportedSchemaError>
  >

const roundtrip = async (s: Schema.Top, v: unknown) => {
  const wc = await compile(s)
  const codec = wc.codec as Schema.Codec<any, any, never, never>
  const wv = await Effect.runPromise(Schema.encodeEffect(codec)(v))
  const back = await Effect.runPromise(Schema.decodeEffect(codec)(wv))
  return { wc, wv, back }
}

describe("Schema.Union → WIT variant", () => {
  it("auto-names cases case0..caseN", async () => {
    const U = Schema.Union([Schema.String, Schema.Number])
    const wc = await compile(U)
    expect(wc.witType.nodes[0]?.type).toMatchObject({
      tag: "variant-type",
      val: [
        ["case0", expect.any(Number)],
        ["case1", expect.any(Number)],
      ],
    })
  })

  it("withVariantCaseName overrides auto-names", async () => {
    const U = Schema.Union([
      Schema.String.pipe(withVariantCaseName("text")),
      Schema.Number.pipe(withVariantCaseName("count")),
    ])
    const wc = await compile(U)
    const root = wc.witType.nodes[0]?.type
    expect(root?.tag).toBe("variant-type")
    expect((root as any).val.map((v: any) => v[0])).toEqual(["text", "count"])
  })

  it("dispatches encode in declaration order, then round-trips", async () => {
    const U = Schema.Union([Schema.String, Int32])
    const a = await roundtrip(U, "hi")
    expect(a.back).toBe("hi")
    expect((a.wv.nodes[0] as any).val[0]).toBe(0)

    const b = await roundtrip(U, 42)
    expect(b.back).toBe(42)
    expect((b.wv.nodes[0] as any).val[0]).toBe(1)
  })

  it("supports unit cases via Null inside a >= 2-real-member union", async () => {
    const U = Schema.Union([
      Schema.String.pipe(withVariantCaseName("text")),
      Schema.Number.pipe(withVariantCaseName("count")),
      Schema.Null.pipe(withVariantCaseName("nothing")),
    ])
    const wc = await compile(U)
    const root = wc.witType.nodes[0]?.type as any
    // 'nothing' has no payload (idx undefined).
    expect(root.val.map((v: any) => v[0])).toEqual(["text", "count", "nothing"])
    expect(root.val[2][1]).toBeUndefined()

    const a = await roundtrip(U, "x")
    expect(a.back).toBe("x")

    // Null members round-trip to null.
    const r = await roundtrip(U, null)
    expect(r.back).toBeNull()
  })

  it("rejects ambiguous unions (two strings)", async () => {
    const U = Schema.Union([Schema.String, Schema.String])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure : null).toBeInstanceOf(UnsupportedSchemaError)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/ambiguous union/)
  })

  it("rejects duplicate object members without _tag", async () => {
    const U = Schema.Union([
      Schema.Struct({ x: Schema.Number }),
      Schema.Struct({ y: Schema.String }),
    ])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/object members/)
  })

  it("rejects duplicate variant case names", async () => {
    const U = Schema.Union([
      Schema.String.pipe(withVariantCaseName("foo")),
      Schema.Number.pipe(withVariantCaseName("foo")),
    ])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/duplicate variant case name/)
  })

  it("still recognises a tagged-union before the generic fallback", async () => {
    const U = Schema.TaggedUnion({
      memory: {},
      local: { path: Schema.String },
    })
    const wc = await compile(U)
    const root = wc.witType.nodes[0]?.type as any
    expect(root.tag).toBe("variant-type")
    expect(root.val.map((v: any) => v[0])).toEqual(["memory", "local"])
    const r = await roundtrip(U, { _tag: "local", path: "/tmp" })
    expect(r.back).toEqual({ _tag: "local", path: "/tmp" })
  })

  it("rejects primitive ⇄ literal overlap", async () => {
    const U = Schema.Union([Schema.String, Schema.Literal("x")])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(
      /primitive .* overlaps a literal/,
    )
  })

  it("rejects mixing plain object with tagged object", async () => {
    const U = Schema.Union([
      Schema.Struct({ x: Schema.Number }),
      Schema.Struct({ _tag: Schema.Literal("named"), name: Schema.String }),
    ])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(
      /mix plain object members with `_tag`/,
    )
  })

  it("rejects unions containing typed arrays (unknown shape)", async () => {
    const { Uint8ArraySchema } = await import("../src/wit-types.js")
    const U = Schema.Union([Uint8ArraySchema as any, Schema.String])
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
    expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/shape cannot be classified/)
  })

  it("Null | Undefined | T does NOT collapse to option, falls to variant", async () => {
    const U = Schema.NullishOr(Schema.String)
    const wc = await compile(U)
    expect(wc.witType.nodes[0]?.type.tag).toBe("variant-type")
    // round-trips for both empties and the real value
    expect((await roundtrip(U, "x")).back).toBe("x")
    expect((await roundtrip(U, null)).back).toBeNull()
    expect((await roundtrip(U, undefined)).back).toBeUndefined()
  })

  it("Schema.Option still emits option-type (not collapsed by toEncoded)", async () => {
    const wc = await compile(Schema.Option(Schema.String))
    expect(wc.witType.nodes[0]?.type.tag).toBe("option-type")
  })

  it("Schema.Result still emits result-type", async () => {
    const wc = await compile(Schema.Result(Schema.Number, Schema.String))
    expect(wc.witType.nodes[0]?.type.tag).toBe("result-type")
  })

  it("rejects optional _tag from being treated as discriminator", async () => {
    const U = Schema.Union([
      Schema.Struct({
        _tag: Schema.optionalKey(Schema.Literal("x")),
        a: Schema.Number,
      }),
      Schema.Struct({
        _tag: Schema.optionalKey(Schema.Literal("y")),
        b: Schema.String,
      }),
    ])
    // Both optional-tag structs become plain objects → rejected as
    // ambiguous plain object members.
    const r = await tryCompile(U)
    expect(Result.isFailure(r)).toBe(true)
  })

  it("mixed union: string | object-with-tag round-trips", async () => {
    const U = Schema.Union([
      Schema.String,
      Schema.Struct({ _tag: Schema.Literal("named"), name: Schema.String }),
    ])
    const a = await roundtrip(U, "raw")
    expect(a.back).toBe("raw")
    const b = await roundtrip(U, { _tag: "named", name: "x" } as const)
    expect(b.back).toEqual({ _tag: "named", name: "x" })
  })
})
