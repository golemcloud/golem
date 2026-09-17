import { describe, expect, it } from "@effect/vitest"
import { Effect, Result, Schema } from "effect"
import { compile, UnsupportedSchemaError } from "../src/WitCodec.js"
import { DiscriminatedUnion, Flags } from "../src/WitTypes.js"

const run = Effect.runPromise

describe("public flags and discriminated-union schemas", () => {
  it("compiles and round-trips flags through the public wire codec", async () => {
    const compiled = await run(compile(Flags(["read", "write"])))
    expect(compiled.graph.root.body).toMatchObject({ tag: "flags", names: ["read", "write"] })
    const encoded = await run(compiled.encode([true, false]))
    expect(await run(compiled.decode(encoded))).toEqual([true, false])
    await expect(run(compiled.encode([true] as any))).rejects.toBeDefined()

    const malformed = structuredClone(encoded)
    ;(malformed.valueNodes[malformed.root] as any).val = [true]
    await expect(run(compiled.decode(malformed))).rejects.toBeDefined()
  })

  it("compiles, selects, and round-trips explicit discriminators", async () => {
    const schema = DiscriminatedUnion([
      {
        tag: "ssh",
        schema: Schema.String.pipe(Schema.check(Schema.isStartsWith("ssh://"))),
        discriminator: { tag: "prefix", val: "ssh://" },
      },
      {
        tag: "circle",
        schema: Schema.Struct({ kind: Schema.Literal("circle"), radius: Schema.Number }),
        discriminator: {
          tag: "field-equals",
          val: { fieldName: "kind", literal: "circle" },
        },
      },
    ])
    const compiled = await run(compile(schema))
    expect(compiled.graph.root.body.tag).toBe("union")
    for (const value of ["ssh://host", { kind: "circle", radius: 2 }] as const) {
      const encoded = await run(compiled.encode(value))
      expect(await run(compiled.decode(encoded))).toEqual(value)
    }
  })

  it("rejects duplicate tags and overlapping discriminators at compilation", async () => {
    const invalid = (branches: Parameters<typeof DiscriminatedUnion>[0]) =>
      run(Effect.result(compile(DiscriminatedUnion(branches))))
    const duplicate = await invalid([
      { tag: "x", schema: Schema.String, discriminator: { tag: "prefix", val: "a" } },
      { tag: "x", schema: Schema.String, discriminator: { tag: "prefix", val: "b" } },
    ])
    const overlap = await invalid([
      { tag: "a", schema: Schema.String, discriminator: { tag: "prefix", val: "a" } },
      { tag: "b", schema: Schema.String, discriminator: { tag: "suffix", val: "b" } },
    ])
    for (const result of [duplicate, overlap]) {
      expect(Result.isFailure(result)).toBe(true)
      expect(Result.isFailure(result) && result.failure).toBeInstanceOf(UnsupportedSchemaError)
    }
  })

  it("rejects asymmetric selected-branch values while decoding", async () => {
    const schema = DiscriminatedUnion([
      { tag: "a", schema: Schema.String, discriminator: { tag: "prefix", val: "a:" } },
      { tag: "b", schema: Schema.String, discriminator: { tag: "prefix", val: "b:" } },
    ])
    const compiled = await run(compile(schema))
    const encoded = await run(compiled.encode("a:value"))
    const node = encoded.valueNodes[encoded.root] as any
    node.val = { tag: "b", body: node.val.body }
    const decoded = await run(Effect.result(compiled.decode(encoded)))
    expect(Result.isFailure(decoded) && decoded.failure).toBeInstanceOf(Schema.SchemaError)
    const unmatched = await run(Effect.result(compiled.encode("c:value")))
    expect(Result.isFailure(unmatched) && unmatched.failure).toBeInstanceOf(Schema.SchemaError)
  })
})
