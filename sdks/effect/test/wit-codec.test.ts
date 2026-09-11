import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/WitCodec.js"

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
  it.effect("round-trips a record (Person) with an optional field present", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Person)
      const value = { name: "Ada", age: 36, alive: true }
      const wv = yield* Schema.encodeEffect(wc.codec)(value)
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips a record (Person) with the optional field absent", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Person)
      const value = { name: "Ada", alive: true }
      const wv = yield* Schema.encodeEffect(wc.codec)(value)
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips a tagged union (Storage), unit and payload cases", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Storage)
      for (const value of [
        { _tag: "memory" as const },
        { _tag: "local" as const, path: "/tmp" },
        { _tag: "s3" as const, bucket: "b", key: "k" },
      ]) {
        const wv = yield* Schema.encodeEffect(wc.codec)(value)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect(back).toEqual(value)
      }
    }),
  )

  it.effect("round-trips a list", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Tags)
      const value = ["a", "b", "c"]
      const wv = yield* Schema.encodeEffect(wc.codec)(value)
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips a primitive string", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Schema.String)
      const wv = yield* Schema.encodeEffect(wc.codec)("hi")
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toBe("hi")
    }),
  )
})
