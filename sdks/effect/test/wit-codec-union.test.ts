import { describe, it, expect } from "@effect/vitest"
import { Effect, Result, Schema } from "effect"
import { toWitCodec, UnsupportedSchemaError } from "../src/wit-codec.js"
import { Int32, withVariantCaseName } from "../src/wit-types.js"

const compile = (s: Schema.Top) => toWitCodec(s as any)

const tryCompile = (s: Schema.Top) =>
  Effect.result(toWitCodec(s as any)) as Effect.Effect<
    Result.Result<unknown, UnsupportedSchemaError>
  >

const roundtrip = (s: Schema.Top, v: unknown) =>
  Effect.gen(function* () {
    const wc = yield* compile(s)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    const wv = yield* Schema.encodeEffect(codec)(v)
    const back = yield* Schema.decodeEffect(codec)(wv)
    return { wc, wv, back }
  })

describe("Schema.Union → WIT variant", () => {
  it.effect("auto-names cases case0..caseN", () =>
    Effect.gen(function* () {
      const U = Schema.Union([Schema.String, Schema.Number])
      const wc = yield* compile(U)
      expect(wc.witType.nodes[0]?.type).toMatchObject({
        tag: "variant-type",
        val: [
          ["case0", expect.any(Number)],
          ["case1", expect.any(Number)],
        ],
      })
    }),
  )

  it.effect("withVariantCaseName overrides auto-names", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.String.pipe(withVariantCaseName("text")),
        Schema.Number.pipe(withVariantCaseName("count")),
      ])
      const wc = yield* compile(U)
      const root = wc.witType.nodes[0]?.type
      expect(root?.tag).toBe("variant-type")
      expect((root as any).val.map((v: any) => v[0])).toEqual(["text", "count"])
    }),
  )

  it.effect("dispatches encode in declaration order, then round-trips", () =>
    Effect.gen(function* () {
      const U = Schema.Union([Schema.String, Int32])
      const a = yield* roundtrip(U, "hi")
      expect(a.back).toBe("hi")
      expect((a.wv.nodes[0] as any).val[0]).toBe(0)

      const b = yield* roundtrip(U, 42)
      expect(b.back).toBe(42)
      expect((b.wv.nodes[0] as any).val[0]).toBe(1)
    }),
  )

  it.effect("supports unit cases via Null inside a >= 2-real-member union", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.String.pipe(withVariantCaseName("text")),
        Schema.Number.pipe(withVariantCaseName("count")),
        Schema.Null.pipe(withVariantCaseName("nothing")),
      ])
      const wc = yield* compile(U)
      const root = wc.witType.nodes[0]?.type as any
      // 'nothing' has no payload (idx undefined).
      expect(root.val.map((v: any) => v[0])).toEqual(["text", "count", "nothing"])
      expect(root.val[2][1]).toBeUndefined()

      const a = yield* roundtrip(U, "x")
      expect(a.back).toBe("x")

      // Null members round-trip to null.
      const r = yield* roundtrip(U, null)
      expect(r.back).toBeNull()
    }),
  )

  it.effect("rejects ambiguous unions (two strings)", () =>
    Effect.gen(function* () {
      const U = Schema.Union([Schema.String, Schema.String])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure : null).toBeInstanceOf(UnsupportedSchemaError)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/ambiguous union/)
    }),
  )

  it.effect("rejects duplicate object members without _tag", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.Struct({ x: Schema.Number }),
        Schema.Struct({ y: Schema.String }),
      ])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/object members/)
    }),
  )

  it.effect("rejects duplicate variant case names", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.String.pipe(withVariantCaseName("foo")),
        Schema.Number.pipe(withVariantCaseName("foo")),
      ])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/duplicate variant case name/)
    }),
  )

  it.effect("still recognises a tagged-union before the generic fallback", () =>
    Effect.gen(function* () {
      const U = Schema.TaggedUnion({
        memory: {},
        local: { path: Schema.String },
      })
      const wc = yield* compile(U)
      const root = wc.witType.nodes[0]?.type as any
      expect(root.tag).toBe("variant-type")
      expect(root.val.map((v: any) => v[0])).toEqual(["memory", "local"])
      const r = yield* roundtrip(U, { _tag: "local", path: "/tmp" })
      expect(r.back).toEqual({ _tag: "local", path: "/tmp" })
    }),
  )

  it.effect("rejects primitive ⇄ literal overlap", () =>
    Effect.gen(function* () {
      const U = Schema.Union([Schema.String, Schema.Literal("x")])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(
        /primitive .* overlaps a literal/,
      )
    }),
  )

  it.effect("rejects mixing plain object with tagged object", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.Struct({ x: Schema.Number }),
        Schema.Struct({ _tag: Schema.Literal("named"), name: Schema.String }),
      ])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(
        /mix plain object members with `_tag`/,
      )
    }),
  )

  it.effect("rejects unions containing typed arrays (unknown shape)", () =>
    Effect.gen(function* () {
      const { Uint8ArraySchema } = yield* Effect.promise(() => import("../src/wit-types.js"))
      const U = Schema.Union([Uint8ArraySchema as any, Schema.String])
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
      expect(Result.isFailure(r) ? r.failure?.reason : null).toMatch(/shape cannot be classified/)
    }),
  )

  it.effect("Null | Undefined | T does NOT collapse to option, falls to variant", () =>
    Effect.gen(function* () {
      const U = Schema.NullishOr(Schema.String)
      const wc = yield* compile(U)
      expect(wc.witType.nodes[0]?.type.tag).toBe("variant-type")
      // round-trips for both empties and the real value
      expect((yield* roundtrip(U, "x")).back).toBe("x")
      expect((yield* roundtrip(U, null)).back).toBeNull()
      expect((yield* roundtrip(U, undefined)).back).toBeUndefined()
    }),
  )

  it.effect("Schema.Option still emits option-type (not collapsed by toEncoded)", () =>
    Effect.gen(function* () {
      const wc = yield* compile(Schema.Option(Schema.String))
      expect(wc.witType.nodes[0]?.type.tag).toBe("option-type")
    }),
  )

  it.effect("Schema.Result still emits result-type", () =>
    Effect.gen(function* () {
      const wc = yield* compile(Schema.Result(Schema.Number, Schema.String))
      expect(wc.witType.nodes[0]?.type.tag).toBe("result-type")
    }),
  )

  it.effect("rejects optional _tag from being treated as discriminator", () =>
    Effect.gen(function* () {
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
      const r = yield* tryCompile(U)
      expect(Result.isFailure(r)).toBe(true)
    }),
  )

  it.effect("mixed union: string | object-with-tag round-trips", () =>
    Effect.gen(function* () {
      const U = Schema.Union([
        Schema.String,
        Schema.Struct({ _tag: Schema.Literal("named"), name: Schema.String }),
      ])
      const a = yield* roundtrip(U, "raw")
      expect(a.back).toBe("raw")
      const b = yield* roundtrip(U, { _tag: "named", name: "x" } as const)
      expect(b.back).toEqual({ _tag: "named", name: "x" })
    }),
  )
})
