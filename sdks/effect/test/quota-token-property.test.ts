import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Layer, Schema } from "effect"
import * as fc from "effect/testing/FastCheck"
import type { QuotaToken as RawQuotaToken } from "golem:core/types@2.0.0"
import { acquireQuotaToken, QuotaTokenSchema } from "../src/Quota.js"
import { QuotaClient } from "../src/host/QuotaClient.js"
import { compile } from "../src/WitCodec.js"

const expectedUseArb = fc.bigInt({ min: 0n, max: 2n ** 64n - 1n })
const resourceNameArb = fc.string({ minLength: 1 })

const layer: Layer.Layer<QuotaClient> = Layer.succeed(
  QuotaClient,
  QuotaClient.of({
    newToken: (resourceName, expectedUse) =>
      Object.freeze({ resourceName, expectedUse }) as unknown as RawQuotaToken,
    reserve: () => ({}) as never,
    commit: () => undefined,
    split: (_token) => Object.freeze({}) as RawQuotaToken,
    merge: () => undefined,
  }),
)

describe("QuotaToken affine codec properties", () => {
  it.effect.prop(
    "a capability transfers to the receiver and each owner can send it only once",
    { resourceName: resourceNameArb, expectedUse: expectedUseArb },
    ({ resourceName, expectedUse }) =>
      Effect.gen(function* () {
        const token = yield* acquireQuotaToken(resourceName, expectedUse)
        const codec = yield* compile(QuotaTokenSchema)

        const wire = yield* codec.encode(token)
        const received = yield* codec.decode(wire)
        yield* codec.encode(received)

        expect(Exit.isFailure(yield* Effect.exit(codec.encode(token)))).toBe(true)
        expect(Exit.isFailure(yield* Effect.exit(codec.encode(received)))).toBe(true)
      }).pipe(Effect.provide(layer)),
  )

  it.effect.prop(
    "a failed enclosing encode rolls capability ownership back",
    { resourceName: resourceNameArb, expectedUse: expectedUseArb },
    ({ resourceName, expectedUse }) =>
      Effect.gen(function* () {
        const token = yield* acquireQuotaToken(resourceName, expectedUse)
        const enclosing = yield* compile(
          Schema.Struct({ token: QuotaTokenSchema, marker: Schema.Literal("ok") }),
        )

        const failed = yield* Effect.exit(enclosing.encode({ token, marker: "not-ok" } as never))
        expect(Exit.isFailure(failed)).toBe(true)

        const tokenCodec = yield* compile(QuotaTokenSchema)
        expect((yield* tokenCodec.encode(token)).valueNodes[0]?.tag).toBe("quota-token-handle")
      }).pipe(Effect.provide(layer)),
  )
})
