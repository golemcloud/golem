import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/wit-codec.js"

const Person = Schema.Struct({
  name: Schema.String,
  age: Schema.optionalKey(Schema.Number),
  alive: Schema.Boolean,
}).pipe(Schema.annotate({ title: "Person" }))

const Storage = Schema.TaggedUnion({
  memory: {},
  local: { path: Schema.String },
  s3: { bucket: Schema.String, key: Schema.String },
}).pipe(Schema.annotate({ title: "Storage" }))

const Tags = Schema.Array(Schema.String)

describe("toWitCodec", () => {
  it("round-trips a record (Person) with an optional field present", async () => {
    const wc = await Effect.runPromise(toWitCodec(Person))
    const value = { name: "Ada", age: 36, alive: true }
    const wv = await Effect.runPromise(Schema.encodeEffect(wc.codec)(value))
    const back = await Effect.runPromise(Schema.decodeEffect(wc.codec)(wv))
    expect(back).toEqual(value)
  })

  it("round-trips a record (Person) with the optional field absent", async () => {
    const wc = await Effect.runPromise(toWitCodec(Person))
    const value = { name: "Ada", alive: true }
    const wv = await Effect.runPromise(Schema.encodeEffect(wc.codec)(value))
    const back = await Effect.runPromise(Schema.decodeEffect(wc.codec)(wv))
    expect(back).toEqual(value)
  })

  it("round-trips a tagged union (Storage), unit and payload cases", async () => {
    const wc = await Effect.runPromise(toWitCodec(Storage))
    for (const value of [
      { _tag: "memory" as const },
      { _tag: "local" as const, path: "/tmp" },
      { _tag: "s3" as const, bucket: "b", key: "k" },
    ]) {
      const wv = await Effect.runPromise(Schema.encodeEffect(wc.codec)(value))
      const back = await Effect.runPromise(Schema.decodeEffect(wc.codec)(wv))
      expect(back).toEqual(value)
    }
  })

  it("round-trips a list", async () => {
    const wc = await Effect.runPromise(toWitCodec(Tags))
    const value = ["a", "b", "c"]
    const wv = await Effect.runPromise(Schema.encodeEffect(wc.codec)(value))
    const back = await Effect.runPromise(Schema.decodeEffect(wc.codec)(wv))
    expect(back).toEqual(value)
  })

  it("round-trips a primitive string", async () => {
    const wc = await Effect.runPromise(toWitCodec(Schema.String))
    const wv = await Effect.runPromise(Schema.encodeEffect(wc.codec)("hi"))
    const back = await Effect.runPromise(Schema.decodeEffect(wc.codec)(wv))
    expect(back).toBe("hi")
  })
})
