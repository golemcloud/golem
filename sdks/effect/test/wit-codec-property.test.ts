import { describe, expect, it } from "@effect/vitest"
import { Effect, Option, Schema } from "effect"
import * as fc from "effect/testing/FastCheck"
import { toWitCodec } from "../src/WitCodec.js"
import { Float64, Int32, Int64 } from "../src/WitTypes.js"

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Asserts `decode(encode(value)) === value` for a single schema/value
 * pair via the compiled `WitCodec`. Inlined here so each property test
 * stays a single Effect chain.
 */
const roundtrip = <S extends Schema.Codec<any, any, never, never>>(schema: S, value: S["Type"]) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(schema)
    const codec = wc.codec as Schema.Codec<S["Type"], any, never, never>
    const wv = yield* Schema.encodeEffect(codec)(value)
    const back = yield* Schema.decodeEffect(codec)(wv)
    expect(back).toEqual(value)
  })

// ---------------------------------------------------------------------------
// Schemas under test
// ---------------------------------------------------------------------------

const Person = Schema.Struct({
  name: Schema.String,
  age: Schema.optionalKey(Int32),
  alive: Schema.Boolean,
})

const Storage = Schema.TaggedUnion({
  memory: {},
  local: { path: Schema.String },
  s3: { bucket: Schema.String, key: Schema.String },
})

const Tags = Schema.Array(Schema.String)

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

/** s32 range. */
const int32Arb = fc.integer({ min: -(2 ** 31), max: 2 ** 31 - 1 })

/** s64 range — wider than JS `number` can express, so we use bigint. */
const int64Arb = fc.bigInt({ min: -(2n ** 63n), max: 2n ** 63n - 1n })

/** Finite f64 — exclude NaN/±Infinity so `toEqual` round-trips cleanly. */
const float64Arb = fc.double({ noNaN: true, noDefaultInfinity: true })

const personArb = fc.record(
  {
    name: fc.string(),
    age: int32Arb,
    alive: fc.boolean(),
  },
  { requiredKeys: ["name", "alive"] },
)

const storageArb = fc.oneof(
  fc.constant({ _tag: "memory" as const }),
  fc.record({ _tag: fc.constant("local" as const), path: fc.string() }),
  fc.record({
    _tag: fc.constant("s3" as const),
    bucket: fc.string(),
    key: fc.string(),
  }),
)

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("WitCodec roundtrip properties", () => {
  it.effect.prop(
    "Schema.String round-trips arbitrary unicode strings",
    { value: fc.string() },
    ({ value }) => roundtrip(Schema.String, value),
  )

  it.effect.prop("Schema.Boolean round-trips both branches", { value: fc.boolean() }, ({ value }) =>
    roundtrip(Schema.Boolean, value),
  )

  it.effect.prop("WitTypes.Int32 round-trips any s32", { value: int32Arb }, ({ value }) =>
    roundtrip(Int32, value),
  )

  it.effect.prop("WitTypes.Int64 round-trips any s64", { value: int64Arb }, ({ value }) =>
    roundtrip(Int64, value),
  )

  it.effect.prop(
    "WitTypes.Float64 round-trips finite doubles",
    { value: float64Arb },
    ({ value }) => roundtrip(Float64, value),
  )

  it.effect.prop(
    "Schema.Array(Schema.String) round-trips any string list",
    { value: fc.array(fc.string()) },
    ({ value }) =>
      Effect.gen(function* () {
        // `Schema.Array` decodes to a `ReadonlyArray<string>`; compare
        // against the same shape.
        const wc = yield* toWitCodec(Tags)
        const wv = yield* Schema.encodeEffect(wc.codec)(value)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect([...back]).toEqual(value)
      }),
  )

  it.effect.prop(
    "Schema.Struct(Person) round-trips with optional field present or absent",
    { value: personArb },
    ({ value }) => roundtrip(Person, value),
  )

  it.effect.prop(
    "Schema.TaggedUnion(Storage) round-trips every case",
    { value: storageArb },
    ({ value }) => roundtrip(Storage, value),
  )

  it.effect.prop(
    "Schema.Option(string) round-trips both none and some",
    { value: fc.option(fc.string(), { nil: undefined }) },
    ({ value }) =>
      roundtrip(
        Schema.Option(Schema.String),
        value === undefined ? Option.none() : Option.some(value),
      ),
  )
})
