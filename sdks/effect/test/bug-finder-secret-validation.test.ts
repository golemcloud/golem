import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer, Schema, SchemaGetter } from "effect"
import type { SchemaValueTree, Secret as RawSecret } from "golem:core/types@2.0.0"
import { vi } from "vitest"
import * as Secrets from "../src/Secrets.js"
import * as WitTypes from "../src/WitTypes.js"
import { SecretsClient } from "../src/host/SecretsClient.js"
import { GuestSecretHandle } from "../src/internal/schema-model/secretHandle.js"
import { decodeFromWire, toWitCodec } from "../src/WitCodec.js"

describe("secret capability validation", () => {
  it("decodes a foreign secret wire node across bundled SDK module copies", async () => {
    vi.resetModules()
    const foreignTypes = await import("../src/WitTypes.js")
    const foreignCodecModule = await import("../src/WitCodec.js")
    const foreignCodec = Effect.runSync(
      foreignCodecModule.compile(foreignTypes.Secret(Schema.String)),
    )

    vi.resetModules()
    const localCodecModule = await import("../src/WitCodec.js")
    const raw = {} as RawSecret
    const wire: SchemaValueTree = {
      root: 0,
      valueNodes: [{ tag: "secret-value", val: raw }],
    }

    const decoded = await Effect.runPromise(
      localCodecModule.decodeFromWire(foreignCodec.codec, wire),
    )
    const returned = await Effect.runPromise(foreignCodec.encode(decoded))
    expect(returned.valueNodes[0]).toEqual({ tag: "secret-value", val: raw })
  })

  it.effect("does not reveal a secret before its enclosing decode commits", () =>
    Effect.gen(function* () {
      const raw = {} as RawSecret
      const wire: SchemaValueTree = {
        root: 0,
        valueNodes: [{ tag: "secret-value", val: raw }],
      }
      let revealCalls = 0
      const host = Layer.succeed(
        SecretsClient,
        SecretsClient.of({
          reveal: () => {
            revealCalls++
            return { root: 0, valueNodes: [{ tag: "string-value", val: "classified" }] }
          },
          id: () => ({}) as never,
          metadata: () => ({}) as never,
        }),
      )
      const base = yield* toWitCodec(WitTypes.Secret(Schema.String))
      const validating = base.codec.pipe(
        Schema.decodeTo(
          Schema.declare((u): u is GuestSecretHandle => u instanceof GuestSecretHandle),
          {
            decode: SchemaGetter.transformEffect((secret) =>
              Effect.gen(function* () {
                const attempted = yield* Effect.result(Secrets.reveal(secret, Schema.String))
                expect(attempted._tag).toBe("Failure")
                return secret
              }),
            ),
            encode: SchemaGetter.transform((secret) => secret),
          },
        ),
      )

      yield* decodeFromWire(validating, wire).pipe(Effect.provide(host))
      expect(revealCalls).toBe(0)
    }),
  )
})
