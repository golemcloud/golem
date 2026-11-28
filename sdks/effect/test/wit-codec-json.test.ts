import { describe, expect, it } from "vitest"
import { Effect, Schema, SchemaGetter } from "effect"
import { compile, compileJson } from "../src/WitCodec.js"
import { SchemaRef, type JsonValue } from "../src/SchemaRef.js"
import { Uint8, Float32, Json } from "../src/WitTypes.js"

describe("compiled canonical JSON", () => {
  it("encodes proxy data properties without reading values or serialization hooks", () => {
    const value = new Proxy(
      { x: [1, null, { y: "nested" }] },
      {
        get() {
          throw new Error("Property getters must not run")
        },
      },
    )
    const codec = Effect.runSync(compile(Json))
    const encoded = Effect.runSync(codec.encode(value))
    expect(Effect.runSync(codec.decode(encoded))).toEqual({ x: [1, null, { y: "nested" }] })
  })

  it("round-trips native JSON through direct wire codecs and inline record fields", async () => {
    const wire = Effect.runSync(compile(Json))
    expect(wire.schemaGraph.typeNodes[wire.schemaGraph.root]?.body.tag).toBe("json-type")
    const record = Effect.runSync(
      compileJson(Schema.Struct({ payload: Json, items: Schema.Array(Json) })),
    )
    for (const value of [
      { b: [1, null, { x: "y" }], a: 1.5 },
      [null, false],
      null,
      "text 😀",
      3.5,
      true,
    ]) {
      const tree = await Effect.runPromise(wire.encodeAsync(value))
      expect(tree.valueNodes[tree.root]?.tag).toBe("json-value")
      expect(await Effect.runPromise(wire.decode(tree))).toEqual(value)
      const input = { payload: value, items: [value] }
      expect(
        await Effect.runPromise(record.encode(await Effect.runPromise(record.decode(input)))),
      ).toEqual(input)
    }
    expect(record.jsonSchema).toMatchObject({ properties: { payload: {}, items: { items: {} } } })
    for (const text of ["{", "1e400", '"\\ud800"', '{"key":"\\udfff"}', '{"\\ud800":true}']) {
      const result = await Effect.runPromise(
        Effect.result(wire.decode({ valueNodes: [{ tag: "json-value", val: text }], root: 0 })),
      )
      expect(result._tag).toBe("Failure")
    }
    for (const text of ['"\\ud800"', '{"key":"\\udfff"}', '{"\\ud800":true}']) {
      expect(Schema.is(Json)(JSON.parse(text))).toBe(false)
    }
    const old = await Effect.runPromise(
      Effect.result(wire.decode({ valueNodes: [{ tag: "string-value", val: "{}" }], root: 0 })),
    )
    expect(old._tag).toBe("Failure")
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    const disguisedSparse = new Array(1)
    Object.defineProperty(disguisedSparse, "4294967295", { value: 7, enumerable: true })
    class HookedArray extends Array<number> {
      toJSON() {
        throw new Error("JSON serialization hooks must not run")
      }
    }
    for (const invalid of [
      undefined,
      1n,
      NaN,
      Infinity,
      { x: undefined },
      [undefined],
      new Array(1),
      disguisedSparse,
      new Date(),
      new HookedArray(1, 2),
      cyclic,
    ])
      expect(Schema.is(Json)(invalid)).toBe(false)
  })

  it("rejects inherited JSON serialization hooks without executing them", () => {
    const codec = Effect.runSync(compile(Json))
    for (const prototype of [Array.prototype, Object.prototype]) {
      const original = Object.getOwnPropertyDescriptor(prototype, "toJSON")
      let hookCalls = 0
      let rejected = false
      try {
        Object.defineProperty(prototype, "toJSON", {
          configurable: true,
          value() {
            hookCalls++
            return "replaced"
          },
        })
        try {
          Effect.runSync(codec.encode([1, 2]))
        } catch {
          rejected = true
        }
      } finally {
        if (original) Object.defineProperty(prototype, "toJSON", original)
        else Reflect.deleteProperty(prototype, "toJSON")
      }
      expect({ rejected, hookCalls }).toEqual({ rejected: true, hookCalls: 0 })
    }
    const value = { toJSON: 1, nested: [1, 2] }
    const tree = Effect.runSync(codec.encode(value))
    expect(Effect.runSync(codec.decode(tree))).toEqual(value)
  })

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
