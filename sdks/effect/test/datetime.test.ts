import { describe, expect, it } from "@effect/vitest"
import { DateTime, Effect, Exit, Schema } from "effect"
import * as Datetime from "../src/Datetime.js"
import { toWitCodec } from "../src/WitCodec.js"

const expectConversionFailure = <A>(effect: Effect.Effect<A, Datetime.DatetimeConversionError>) =>
  Effect.gen(function* () {
    const exit = yield* Effect.exit(effect)
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      expect(String(exit.cause)).toContain("DatetimeConversionError")
    }
  })

describe("Datetime", () => {
  it.effect("uses the WASI u64/u32 wire shape and preserves nanosecond precision", () =>
    Effect.gen(function* () {
      const codec = yield* toWitCodec(Datetime.Datetime)
      const root = codec.graph.root.body
      expect(root.tag).toBe("record")
      if (root.tag !== "record") throw new Error()
      const seconds = root.fields.find(({ name }) => name === "seconds")!
      const nanoseconds = root.fields.find(({ name }) => name === "nanoseconds")!
      expect(seconds.body.body.tag).toBe("u64")
      expect(nanoseconds.body.body.tag).toBe("u32")

      const value = { seconds: 42n, nanoseconds: 123_456_789 }
      const wire = yield* Schema.encodeEffect(codec.codec)(value)
      expect(yield* Schema.decodeEffect(codec.codec)(wire)).toEqual(value)
    }),
  )

  it.effect("validates the WIT seconds and nanoseconds ranges", () =>
    Effect.gen(function* () {
      yield* expectConversionFailure(Datetime.fromInput({ seconds: -1n, nanoseconds: 0 } as any))
      yield* expectConversionFailure(
        Datetime.fromInput({ seconds: 2n ** 64n, nanoseconds: 0 } as any),
      )
      yield* expectConversionFailure(
        Datetime.fromInput({ seconds: 0n, nanoseconds: 1_000_000_000 } as any),
      )
      yield* expectConversionFailure(Datetime.fromInput({ seconds: 0n, nanoseconds: 0.5 } as any))
    }),
  )

  it.effect("round-trips epoch milliseconds, Date, and UTC DateTime", () =>
    Effect.gen(function* () {
      const epochMilliseconds = 1_700_000_000_123
      const expected = { seconds: 1_700_000_000n, nanoseconds: 123_000_000 }

      expect(yield* Datetime.fromEpochMilliseconds(epochMilliseconds)).toEqual(expected)
      expect(yield* Datetime.fromDate(new Date(epochMilliseconds))).toEqual(expected)

      const effectDateTime = DateTime.fromDateUnsafe(new Date(epochMilliseconds))
      expect(yield* Datetime.fromDateTime(effectDateTime)).toEqual(expected)
      expect(yield* Datetime.toEpochMilliseconds(expected)).toBe(epochMilliseconds)
      expect((yield* Datetime.toDate(expected)).getTime()).toBe(epochMilliseconds)
      expect((yield* Datetime.toDateTime(expected)).epochMilliseconds).toBe(epochMilliseconds)
    }),
  )

  it.effect("uses the absolute instant of zoned Effect DateTime values", () =>
    Effect.gen(function* () {
      const epochMilliseconds = 1_700_000_000_123
      const zoned = DateTime.makeZonedUnsafe(epochMilliseconds, {
        timeZone: "Pacific/Auckland",
      })
      expect(yield* Datetime.fromDateTime(zoned)).toEqual({
        seconds: 1_700_000_000n,
        nanoseconds: 123_000_000,
      })
    }),
  )

  it.effect("rejects negative timestamps in every millisecond-based input form", () =>
    Effect.gen(function* () {
      yield* expectConversionFailure(Datetime.fromEpochMilliseconds(-1))
      yield* expectConversionFailure(Datetime.fromDate(new Date(-1)))
      yield* expectConversionFailure(Datetime.fromDateTime(DateTime.makeUnsafe(-1)))
    }),
  )

  it.effect("rejects invalid and non-integral millisecond inputs", () =>
    Effect.gen(function* () {
      yield* expectConversionFailure(Datetime.fromDate(new Date(Number.NaN)))
      yield* expectConversionFailure(Datetime.fromEpochMilliseconds(Number.NaN))
      yield* expectConversionFailure(Datetime.fromEpochMilliseconds(Number.POSITIVE_INFINITY))
      yield* expectConversionFailure(Datetime.fromEpochMilliseconds(0.5))
      yield* expectConversionFailure(
        Datetime.fromDateTime(DateTime.makeUnsafe({ epochMilliseconds: Number.NaN })),
      )
    }),
  )

  it.effect("rejects sub-millisecond conversion instead of truncating", () =>
    Effect.gen(function* () {
      const value = { seconds: 1n, nanoseconds: 123_456_789 }
      yield* expectConversionFailure(Datetime.toEpochMilliseconds(value))
      yield* expectConversionFailure(Datetime.toDate(value))
      yield* expectConversionFailure(Datetime.toDateTime(value))
    }),
  )

  it.effect("rejects values outside the JavaScript Date range", () =>
    Effect.gen(function* () {
      yield* expectConversionFailure(Datetime.fromEpochMilliseconds(8_640_000_000_000_001))
      yield* expectConversionFailure(
        Datetime.toEpochMilliseconds({ seconds: 8_640_000_000_001n, nanoseconds: 0 }),
      )
    }),
  )

  it("exports the canonical Datetime schema from its owning module", () => {
    expect(Datetime.Datetime.fields.seconds).toBeDefined()
    expect(Datetime.Datetime.fields.nanoseconds).toBeDefined()
  })
})
