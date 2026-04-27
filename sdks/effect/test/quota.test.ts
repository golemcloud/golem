import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import { QuotaToken as QuotaTokenSchema, QuotaTokenRecord } from "../src/quota.js"
import { toWitCodec } from "../src/wit-codec.js"
import { QuotaToken } from "golem:quota/types@1.5.0"

const sample = {
  environmentId: { uuid: { highBits: 1n, lowBits: 2n } },
  resourceName: "cpu",
  expectedUse: 100n,
  lastCredit: 50n,
  lastCreditAt: { seconds: 1_700_000_000n, nanoseconds: 123 },
}

describe("QuotaToken schema", () => {
  it("encodes a QuotaToken instance to its record shape", async () => {
    const token = QuotaToken.fromRecord(sample)
    const enc = await Effect.runPromise(Schema.encodeEffect(QuotaTokenSchema)(token))
    expect(enc).toEqual(sample)
  })

  it("decodes a record into a QuotaToken instance", async () => {
    const token = await Effect.runPromise(Schema.decodeEffect(QuotaTokenSchema)(sample))
    expect(token).toBeInstanceOf(QuotaToken)
    expect(token.toRecord()).toEqual(sample)
  })

  it("compiles to a WIT record with the expected field types", async () => {
    const wc = await Effect.runPromise(toWitCodec(QuotaTokenSchema as any))
    const root = wc.witType.nodes[0]?.type as any
    expect(root.tag).toBe("record-type")
    const fieldNames = root.val.map((p: [string, number]) => p[0])
    expect(fieldNames).toEqual([
      "environmentId",
      "resourceName",
      "expectedUse",
      "lastCredit",
      "lastCreditAt",
    ])

    // expectedUse must be u64; lastCredit is s64; lastCreditAt.nanoseconds is u32.
    const find = (name: string) => root.val.find((p: [string, number]) => p[0] === name)[1]
    expect(wc.witType.nodes[find("expectedUse")]?.type.tag).toBe("prim-u64-type")
    expect(wc.witType.nodes[find("lastCredit")]?.type.tag).toBe("prim-s64-type")
  })

  it("round-trips QuotaToken end-to-end through the WIT codec", async () => {
    const wc = await Effect.runPromise(toWitCodec(QuotaTokenSchema as any))
    const codec = wc.codec as Schema.Codec<QuotaToken, any, never, never>
    const token = QuotaToken.fromRecord(sample)
    const wv = await Effect.runPromise(Schema.encodeEffect(codec)(token))
    const back = await Effect.runPromise(Schema.decodeEffect(codec)(wv))
    expect(back).toBeInstanceOf(QuotaToken)
    expect(back.toRecord()).toEqual(sample)
  })

  it("QuotaTokenRecord schema matches the host record shape", async () => {
    const rec = await Effect.runPromise(Schema.decodeEffect(QuotaTokenRecord)(sample))
    expect(rec).toEqual(sample)
  })
})
