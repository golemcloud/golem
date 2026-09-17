import { describe, expect, it } from "@effect/vitest"
import { Deferred, Effect, Fiber, Layer, Schema, SchemaGetter } from "effect"
import type { QuotaToken as RawQuotaToken, SchemaValueTree } from "golem:core/types@2.0.0"
import { acquireQuotaToken, merge, QuotaToken, QuotaTokenSchema } from "../src/Quota.js"
import { QuotaClient } from "../src/host/QuotaClient.js"
import { compile, decodeFromWire, toWitCodec } from "../src/WitCodec.js"

describe("WitCodec quota-token ownership", () => {
  it.effect("restores a token and its wire owner after sibling validation fails", () =>
    Effect.gen(function* () {
      const raw = {} as RawQuotaToken
      const wire: SchemaValueTree = {
        root: 0,
        valueNodes: [
          { tag: "record-value", val: [1, 2] },
          { tag: "quota-token-handle", val: raw },
          { tag: "string-value", val: "invalid" },
        ],
      }
      const codec = yield* compile(
        Schema.Struct({ token: QuotaTokenSchema, name: Schema.Literal("valid") }),
      )
      expect((yield* Effect.result(codec.decode(wire)))._tag).toBe("Failure")
      expect(wire.valueNodes[1]).toEqual({ tag: "quota-token-handle", val: raw })
      wire.valueNodes[2] = { tag: "string-value", val: "valid" }
      const received = yield* codec.decode(wire)
      expect(
        (yield* codec.encode(received)).valueNodes.find(
          (node) => node.tag === "quota-token-handle",
        ),
      ).toEqual({
        tag: "quota-token-handle",
        val: raw,
      })
    }),
  )

  it.effect("blocks transfer during suspended validation and rolls back on interruption", () =>
    Effect.gen(function* () {
      const raw = {} as RawQuotaToken
      const wire: SchemaValueTree = {
        root: 0,
        valueNodes: [{ tag: "quota-token-handle", val: raw }],
      }
      const entered = yield* Deferred.make<void>()
      const base = yield* toWitCodec(QuotaTokenSchema)
      const transport = yield* compile(QuotaTokenSchema)
      const delayed = base.codec.pipe(
        Schema.decodeTo(
          Schema.declare((u): u is QuotaToken => u instanceof QuotaToken),
          {
            decode: SchemaGetter.transformOrFail((token) =>
              Effect.gen(function* () {
                expect((yield* Effect.result(transport.encode(token)))._tag).toBe("Failure")
                yield* Deferred.succeed(entered, undefined)
                return yield* Effect.never
              }),
            ),
            encode: SchemaGetter.transform((token) => token),
          },
        ),
      )
      const fiber = yield* Effect.forkChild(decodeFromWire(delayed, wire))
      yield* Deferred.await(entered)
      yield* Fiber.interrupt(fiber)
      expect(wire.valueNodes[0]).toEqual({ tag: "quota-token-handle", val: raw })
      const received = yield* transport.decode(wire)
      expect((yield* transport.encode(received)).valueNodes[0]).toEqual({
        tag: "quota-token-handle",
        val: raw,
      })
    }),
  )

  it.effect("lowers a raw token that is already owned by its public wrapper", () =>
    Effect.gen(function* () {
      const raw = {} as RawQuotaToken
      const token = yield* acquireQuotaToken("test", 1n).pipe(
        Effect.provide(
          Layer.succeed(
            QuotaClient,
            QuotaClient.of({
              newToken: () => raw,
              reserve: () => ({}) as never,
              commit: () => undefined,
              split: () => raw,
              merge: () => undefined,
            }),
          ),
        ),
      )
      const codec = yield* compile(QuotaTokenSchema)

      const wire = yield* codec.encode(token)
      expect(wire.valueNodes).toEqual([{ tag: "quota-token-handle", val: raw }])
      const received = yield* codec.decode(wire)
      expect((yield* codec.encode(received)).valueNodes).toEqual([
        { tag: "quota-token-handle", val: raw },
      ])
      expect((yield* Effect.exit(codec.encode(token)))._tag).toBe("Failure")
      expect((yield* Effect.exit(codec.encode(received)))._tag).toBe("Failure")
    }),
  )

  it.effect("does not resurrect a token after a consuming host merge fails", () =>
    Effect.gen(function* () {
      let calls = 0
      const host = QuotaClient.of({
        newToken: () => ({}) as RawQuotaToken,
        reserve: () => ({}) as never,
        commit: () => undefined,
        split: () => ({}) as RawQuotaToken,
        merge: () => {
          calls++
          throw new Error("host rejected after consuming source")
        },
      })
      const program = Effect.gen(function* () {
        const target = yield* acquireQuotaToken("test", 1n)
        const source = yield* acquireQuotaToken("test", 2n)
        expect((yield* Effect.result(merge(target, source)))._tag).toBe("Failure")
        expect((yield* Effect.result(merge(target, source)))._tag).toBe("Failure")
        expect(calls).toBe(1)
      })
      yield* program.pipe(Effect.provideService(QuotaClient, host))
    }),
  )
})
