import { describe, it, expect } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { toWitCodec } from "../src/WitCodec.js"
import { PrincipalSchema, type PrincipalValue } from "../src/Principal.js"

const roundtrip = (value: PrincipalValue) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(PrincipalSchema as any)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    const sv = yield* Schema.encodeEffect(codec)(value)
    const back = yield* Schema.decodeEffect(codec)(sv)
    return { wc, sv, back }
  })

describe("PrincipalSchema → principal variant", () => {
  it.effect("emits a 4-case variant root (oidc/agent/golem-user/anonymous)", () =>
    Effect.gen(function* () {
      const { wc } = yield* roundtrip({ tag: "anonymous" })
      expect(wc.graph.root.body.tag).toBe("variant")
      const cases = (wc.graph.root.body as { tag: "variant"; cases: Array<{ name: string }> }).cases
      expect(cases.map((c) => c.name)).toEqual(["oidc", "agent", "golem-user", "anonymous"])
    }),
  )

  it.effect("round-trips a full OidcPrincipal", () =>
    Effect.gen(function* () {
      const value: PrincipalValue = {
        tag: "oidc",
        val: {
          sub: "user-123",
          issuer: "https://issuer.example",
          email: "a@example.test",
          name: "Ada",
          emailVerified: true,
          givenName: "Ada",
          familyName: "Lovelace",
          picture: "https://pics.example/ada.png",
          preferredUsername: "ada",
          claims: '{"role":"admin"}',
        },
      }
      const { sv, back } = yield* roundtrip(value)
      expect(sv.tag).toBe("variant")
      expect((sv as { caseIndex: number }).caseIndex).toBe(0)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips a sparse OidcPrincipal (optional fields omitted)", () =>
    Effect.gen(function* () {
      const value: PrincipalValue = {
        tag: "oidc",
        val: { sub: "s", issuer: "i", claims: "{}" },
      }
      const { back } = yield* roundtrip(value)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips an anonymous principal", () =>
    Effect.gen(function* () {
      const value: PrincipalValue = { tag: "anonymous" }
      const { sv, back } = yield* roundtrip(value)
      expect((sv as { caseIndex: number }).caseIndex).toBe(3)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips an agent principal", () =>
    Effect.gen(function* () {
      const value: PrincipalValue = {
        tag: "agent",
        val: {
          agentId: {
            componentId: { uuid: { highBits: 1n, lowBits: 2n } },
            agentId: "counter/foo",
          },
        },
      }
      const { sv, back } = yield* roundtrip(value)
      expect((sv as { caseIndex: number }).caseIndex).toBe(1)
      expect(back).toEqual(value)
    }),
  )

  it.effect("round-trips a golem-user principal", () =>
    Effect.gen(function* () {
      const value: PrincipalValue = {
        tag: "golem-user",
        val: { accountId: { uuid: { highBits: 7n, lowBits: 8n } } },
      }
      const { sv, back } = yield* roundtrip(value)
      expect((sv as { caseIndex: number }).caseIndex).toBe(2)
      expect(back).toEqual(value)
    }),
  )
})
