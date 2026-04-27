import { describe, it, expect } from "vitest"
import { Effect, HashMap, Option, Result, Schema } from "effect"
import { toWitCodec } from "../src/wit-codec.js"
import {
  Float32,
  Int8,
  Int16,
  Int32,
  Int64,
  Uint8,
  Uint16,
  Uint32,
  Uint64,
} from "../src/wit-types.js"

const roundtrip = async <S extends Schema.Codec<any, any, never, never>>(
  s: S,
  value: S["Type"],
) => {
  const wc = await Effect.runPromise(toWitCodec(s))
  const wv = await Effect.runPromise(
    Schema.encodeEffect(wc.codec as Schema.Codec<S["Type"], any, never, never>)(value),
  )
  const back = await Effect.runPromise(
    Schema.decodeEffect(wc.codec as Schema.Codec<S["Type"], any, never, never>)(wv),
  )
  return { wc, wv, back }
}

describe("toWitCodec — option-shaped types", () => {
  it("NullOr<string>: option<string> with null/value", async () => {
    const S = Schema.NullOr(Schema.String)
    const a = await roundtrip(S, "hi")
    expect(a.back).toBe("hi")
    expect(a.wc.witType.nodes[0]?.type.tag).toBe("option-type")

    const b = await roundtrip(S, null)
    expect(b.back).toBeNull()
  })

  it("UndefinedOr<number>: option<f64> with undefined/value", async () => {
    const S = Schema.UndefinedOr(Schema.Number)
    const a = await roundtrip(S, 42)
    expect(a.back).toBe(42)

    const b = await roundtrip(S, undefined)
    expect(b.back).toBeUndefined()
  })

  it("Schema.Option<string>: option<string> with Effect Option", async () => {
    const S = Schema.Option(Schema.String)
    const a = await roundtrip(S, Option.some("hi"))
    expect(Option.isSome(a.back)).toBe(true)
    expect((a.back as Option.Option<string>).pipe(Option.getOrThrow)).toBe("hi")

    const b = await roundtrip(S, Option.none())
    expect(Option.isNone(b.back)).toBe(true)
    expect(b.wc.witType.nodes[0]?.type.tag).toBe("option-type")
  })
})

describe("toWitCodec — Result", () => {
  it("Result<number, string> as success and failure", async () => {
    const S = Schema.Result(Schema.Number, Schema.String)
    const ok = await roundtrip(S, Result.succeed(7))
    expect(Result.isSuccess(ok.back)).toBe(true)
    expect((ok.back as Result.Result<number, string>).pipe(Result.getOrThrow)).toBe(7)
    expect(ok.wc.witType.nodes[0]?.type.tag).toBe("result-type")

    const err = await roundtrip(S, Result.fail("nope"))
    expect(Result.isFailure(err.back)).toBe(true)
    expect(err.back as Result.Result<number, string>).toMatchObject({
      _tag: "Failure",
      failure: "nope",
    })
  })
})

describe("toWitCodec — Maps", () => {
  it("ReadonlyMap<string, number> as list<tuple<string, f64>>", async () => {
    const S = Schema.ReadonlyMap(Schema.String, Schema.Number)
    const value = new Map<string, number>([
      ["a", 1],
      ["b", 2],
    ])
    const r = await roundtrip(S, value)
    expect(r.back).toBeInstanceOf(Map)
    expect(Array.from((r.back as Map<string, number>).entries())).toEqual([
      ["a", 1],
      ["b", 2],
    ])
    // Root is list<tuple<k, v>>
    expect(r.wc.witType.nodes[0]?.type.tag).toBe("list-type")
  })

  it("HashMap<string, number> round-trips and stays a HashMap", async () => {
    const S = Schema.HashMap(Schema.String, Schema.Number)
    const value: HashMap.HashMap<string, number> = HashMap.fromIterable([
      ["a", 1] as const,
      ["b", 2] as const,
    ])
    const r = await roundtrip(S, value)
    expect(HashMap.isHashMap(r.back)).toBe(true)
    expect(HashMap.size(r.back as HashMap.HashMap<string, number>)).toBe(2)
    expect(HashMap.get(r.back as HashMap.HashMap<string, number>, "a").pipe(Option.getOrNull)).toBe(
      1,
    )
  })
})

describe("toWitCodec — sized integer schemas", () => {
  const cases: ReadonlyArray<{
    readonly name: string
    readonly schema: Schema.Top
    readonly typeTag: string
    readonly valueTag: string
    readonly value: number | bigint
  }> = [
    { name: "Uint8", schema: Uint8, typeTag: "prim-u8-type", valueTag: "prim-u8", value: 200 },
    {
      name: "Uint16",
      schema: Uint16,
      typeTag: "prim-u16-type",
      valueTag: "prim-u16",
      value: 65000,
    },
    {
      name: "Uint32",
      schema: Uint32,
      typeTag: "prim-u32-type",
      valueTag: "prim-u32",
      value: 4_000_000_000,
    },
    { name: "Int8", schema: Int8, typeTag: "prim-s8-type", valueTag: "prim-s8", value: -100 },
    { name: "Int16", schema: Int16, typeTag: "prim-s16-type", valueTag: "prim-s16", value: -32000 },
    {
      name: "Int32",
      schema: Int32,
      typeTag: "prim-s32-type",
      valueTag: "prim-s32",
      value: -2_000_000_000,
    },
    {
      name: "Float32",
      schema: Float32,
      typeTag: "prim-f32-type",
      valueTag: "prim-float32",
      value: 1.5,
    },
    {
      name: "Int64",
      schema: Int64,
      typeTag: "prim-s64-type",
      valueTag: "prim-s64",
      value: -9_000_000_000_000n,
    },
    {
      name: "Uint64",
      schema: Uint64,
      typeTag: "prim-u64-type",
      valueTag: "prim-u64",
      value: 9_000_000_000_000n,
    },
  ]

  for (const c of cases) {
    it(`${c.name} maps to ${c.typeTag} and round-trips`, async () => {
      const wc = await Effect.runPromise(toWitCodec(c.schema as any))
      expect(wc.witType.nodes[0]?.type.tag).toBe(c.typeTag)
      const codec = wc.codec as Schema.Codec<any, any, never, never>
      const wv = await Effect.runPromise(Schema.encodeEffect(codec)(c.value))
      expect(wv.nodes[0]?.tag).toBe(c.valueTag)
      const back = await Effect.runPromise(Schema.decodeEffect(codec)(wv))
      expect(back).toEqual(c.value)
    })
  }

  it("default Schema.Number stays f64", async () => {
    const wc = await Effect.runPromise(toWitCodec(Schema.Number))
    expect(wc.witType.nodes[0]?.type.tag).toBe("prim-f64-type")
  })

  it("default Schema.BigInt stays s64", async () => {
    const wc = await Effect.runPromise(toWitCodec(Schema.BigInt))
    expect(wc.witType.nodes[0]?.type.tag).toBe("prim-s64-type")
  })
})

describe("toWitCodec — composite uses of new types", () => {
  it("Result<ReadonlyMap<string, Uint32>, string>", async () => {
    const S = Schema.Result(Schema.ReadonlyMap(Schema.String, Uint32), Schema.String)
    const value = Result.succeed(new Map([["x", 7]]))
    const r = await roundtrip(S, value)
    expect(Result.isSuccess(r.back)).toBe(true)
    const m = (r.back as Result.Result<Map<string, number>, string>).pipe(Result.getOrThrow)
    expect(Array.from(m.entries())).toEqual([["x", 7]])
  })

  it("Struct with NullOr and Schema.Option fields", async () => {
    const S = Schema.Struct({
      name: Schema.String,
      nick: Schema.NullOr(Schema.String),
      age: Schema.Option(Uint8),
    })
    const v = { name: "Ada", nick: null, age: Option.some(36) }
    const r = await roundtrip(S, v)
    expect(r.back).toMatchObject({ name: "Ada", nick: null })
    expect(Option.getOrNull((r.back as { age: Option.Option<number> }).age)).toBe(36)
  })
})
