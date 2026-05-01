import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import * as fc from "effect/testing/FastCheck"
import { QuotaToken, QuotaTokenRecord } from "../src/Quota.js"

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

/** UUIDs are `{ highBits: u64, lowBits: u64 }` of bigints in the u64 range. */
const u64Arb = fc.bigInt({ min: 0n, max: 2n ** 64n - 1n })
const i64Arb = fc.bigInt({ min: -(2n ** 63n), max: 2n ** 63n - 1n })
const u32Arb = fc.integer({ min: 0, max: 2 ** 32 - 1 })

const uuidArb = fc.record({ highBits: u64Arb, lowBits: u64Arb })

/**
 * A well-formed `QuotaTokenRecord` (the wire shape produced by
 * `QuotaToken.toRecord()`).
 */
const recordArb = fc.record({
  environmentId: fc.record({ uuid: uuidArb }),
  resourceName: fc.string(),
  expectedUse: u64Arb,
  lastCredit: i64Arb,
  lastCreditAt: fc.record({ seconds: i64Arb, nanoseconds: u32Arb }),
})

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("QuotaToken codec roundtrip properties", () => {
  it.effect.prop(
    "QuotaTokenRecord -> QuotaToken instance -> QuotaTokenRecord is identity",
    { record: recordArb },
    ({ record }) =>
      Effect.gen(function* () {
        // Sanity: the generated record validates against the schema.
        const validated = yield* Schema.decodeUnknownEffect(QuotaTokenRecord)(record)
        // Decode through the host-class codec, then re-encode.
        const instance = yield* Schema.decodeEffect(QuotaToken)(validated)
        const back = yield* Schema.encodeEffect(QuotaToken)(instance)
        expect(back).toEqual(validated)
      }),
  )

  it.effect.prop(
    "QuotaTokenRecord schema validates the generator output",
    { record: recordArb },
    ({ record }) =>
      Effect.gen(function* () {
        const r = yield* Schema.decodeUnknownEffect(QuotaTokenRecord)(record)
        // The schema is structural; the only thing decode does is
        // assert types. Encode is also identity.
        const back = yield* Schema.encodeEffect(QuotaTokenRecord)(r)
        expect(back).toEqual(record)
      }),
  )
})
