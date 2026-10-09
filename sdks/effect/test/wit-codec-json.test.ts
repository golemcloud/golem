import { describe, expect, it } from "vitest"
import { Effect, Schema, SchemaGetter } from "effect"
import { compile, compileJson } from "../src/WitCodec.js"
import { SchemaRef, type JsonValue } from "../src/SchemaRef.js"
import { Uint8, Float32 } from "../src/WitTypes.js"

describe("compiled canonical JSON", () => {
  it("matches the wire boundary for nested optional values and f32 values", async () => {
    const schema = Schema.Struct({
      bytes: Schema.Array(Uint8),
      optional: Schema.optionalKey(Schema.String),
      rounded: Float32,
    })
    const codec = Effect.runSync(compileJson(schema))
    const wire = Effect.runSync(compile(schema))
    const ref = new SchemaRef(wire.schemaGraph)
    const valid: JsonValue[] = [
      { bytes: [7, 255], rounded: 1.1 },
      { bytes: [31], optional: "present", rounded: -0 },
      { bytes: [7], optional: null, rounded: 1 },
    ]
    for (const value of valid) {
      expect(await Effect.runPromise(codec.decode(value))).toEqual(
        await Effect.runPromise(wire.decode(ref.packJson(value))),
      )
      const application = await Effect.runPromise(codec.decode(value))
      expect(await Effect.runPromise(codec.encode(application))).toEqual(
        ref.unpackJson(await Effect.runPromise(wire.encodeAsync(application))),
      )
    }
    const invalid: JsonValue[] = [
      { bytes: [256], rounded: 1 },
      { bytes: [7], rounded: Infinity },
    ]
    for (const value of invalid) {
      expect(Effect.runSync(Effect.result(codec.decode(value)))).toMatchObject({ _tag: "Failure" })
      expect(() => ref.packJson(value)).toThrow()
    }
  })

  it("executes stateful schema transforms exactly once on every call", async () => {
    let decoded = 0
    let encoded = 0
    const schema = Schema.String.pipe(
      Schema.decodeTo(Schema.String, {
        decode: SchemaGetter.transform((value) => {
          decoded++
          return `${value}!`
        }),
        encode: SchemaGetter.transform((value) => {
          encoded++
          return value.slice(0, -1)
        }),
      }),
    )
    const codec = Effect.runSync(compileJson(schema))
    for (let i = 0; i < 3; i++) {
      expect(await Effect.runPromise(codec.decode("value"))).toBe("value!")
      expect(await Effect.runPromise(codec.encode("value!"))).toBe("value")
    }
    expect(decoded).toBe(3)
    expect(encoded).toBe(3)
  })
})
