import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Schema } from "effect"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint8, Uint64, restrict } from "../src/WitTypes.js"

const bodyOf = (s: Schema.Top) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(s as any)
    return wc.graph.root.body as any
  })

describe("numeric restrictions", () => {
  it.effect("Uint8.pipe(restrict({min,max})) carries unsigned restrictions + round-trips", () =>
    Effect.gen(function* () {
      const s = Uint8.pipe(restrict({ min: 1, max: 200 }))
      const body = yield* bodyOf(s)
      expect(body.tag).toBe("u8")
      expect(body.restrictions?.min).toEqual({ tag: "unsigned", val: 1n })
      expect(body.restrictions?.max).toEqual({ tag: "unsigned", val: 200n })
      const wc = yield* toWitCodec(s as any)
      const sv = yield* Schema.encodeEffect(wc.codec as any)(50)
      expect(yield* Schema.decodeEffect(wc.codec as any)(sv)).toBe(50)
    }),
  )

  it.effect("Schema.Number.pipe(restrict({max})) -> f64 float-bits restriction", () =>
    Effect.gen(function* () {
      const body = yield* bodyOf(Schema.Number.pipe(restrict({ max: 9 })))
      expect(body.tag).toBe("f64")
      expect(body.restrictions?.max?.tag).toBe("float-bits")
    }),
  )

  it.effect("a bare Uint8 has no restrictions", () =>
    Effect.gen(function* () {
      const body = yield* bodyOf(Uint8)
      expect(body.tag).toBe("u8")
      expect(body.restrictions).toBeUndefined()
    }),
  )

  it.effect("integer pins reject out-of-range and non-integer values (decode)", () =>
    Effect.gen(function* () {
      expect(yield* Schema.decodeUnknownEffect(Uint8)(200)).toBe(200)
      for (const bad of [999, -1, 3.7]) {
        const exit = yield* Effect.exit(Schema.decodeUnknownEffect(Uint8)(bad))
        expect(Exit.isFailure(exit)).toBe(true)
      }
    }),
  )

  it.effect("restrict tightens the range and rejects out-of-bound values", () =>
    Effect.gen(function* () {
      const s = Uint8.pipe(restrict({ min: 10, max: 20 }))
      expect(yield* Schema.decodeUnknownEffect(s)(15)).toBe(15)
      for (const bad of [9, 21]) {
        const exit = yield* Effect.exit(Schema.decodeUnknownEffect(s)(bad))
        expect(Exit.isFailure(exit)).toBe(true)
      }
    }),
  )

  it.effect("bigint pins (Uint64) reject negatives", () =>
    Effect.gen(function* () {
      expect(yield* Schema.decodeUnknownEffect(Uint64)(42n)).toBe(42n)
      const exit = yield* Effect.exit(Schema.decodeUnknownEffect(Uint64)(-1n))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("the wit codec enforces the range on encode (invocation boundary)", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Uint8 as any)
      // in-range encodes; out-of-range is rejected at the codec boundary
      yield* Schema.encodeEffect(wc.codec as any)(200)
      const exit = yield* Effect.exit(Schema.encodeEffect(wc.codec as any)(999))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )
})
