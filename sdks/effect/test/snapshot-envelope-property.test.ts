import { describe, expect, it } from "@effect/vitest"
import { Schema } from "effect"
import * as fc from "effect/testing/FastCheck"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  encodeMultipartJsonEnvelope,
  serializePrincipal,
} from "../src/internal/snapshotEnvelope.js"

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

/**
 * Build a `Principal` (runtime form, with bigint UUIDs) from canonical
 * UUID strings. We hand-roll the arbitrary instead of relying on
 * `Schema.toArbitrary(SerializedPrincipal)` because the JSON shape's
 * `componentId` / `accountId` slots are typed as `Schema.String`, but
 * `deserializePrincipal` round-trips them through `parseUuid` which
 * requires the canonical 8-4-4-4-12 hex form.
 */
const principalArb: fc.Arbitrary<AgentCommon.Principal> = fc.oneof(
  fc.constant({ tag: "anonymous" } as const satisfies AgentCommon.Principal),
  fc.record({ componentId: fc.uuid(), agentId: fc.string() }).map(
    (v) =>
      ({
        tag: "agent",
        val: {
          agentId: {
            componentId: { uuid: parseUuid(v.componentId) },
            agentId: v.agentId,
          },
        },
      }) satisfies AgentCommon.Principal,
  ),
  fc.record({ accountId: fc.uuid() }).map(
    (v) =>
      ({
        tag: "golem-user",
        val: { accountId: { uuid: parseUuid(v.accountId) } },
      }) satisfies AgentCommon.Principal,
  ),
  fc
    .record(
      {
        sub: fc.string(),
        issuer: fc.webUrl(),
        email: fc.option(fc.emailAddress(), { nil: undefined }),
        name: fc.option(fc.string(), { nil: undefined }),
        emailVerified: fc.option(fc.boolean(), { nil: undefined }),
        givenName: fc.option(fc.string(), { nil: undefined }),
        familyName: fc.option(fc.string(), { nil: undefined }),
        picture: fc.option(fc.webUrl(), { nil: undefined }),
        preferredUsername: fc.option(fc.string(), { nil: undefined }),
        claims: fc.string(),
      },
      { requiredKeys: ["sub", "issuer", "claims"] },
    )
    .map(
      (val) =>
        ({
          tag: "oidc",
          val,
        }) satisfies AgentCommon.Principal,
    ),
)

/**
 * Re-implement parseUuid locally so the arbitrary stays self-contained
 * (and the test does not need to import the host alias module twice).
 */
function parseUuid(uuid: string): { highBits: bigint; lowBits: bigint } {
  const hex = uuid.replace(/-/g, "")
  return {
    highBits: BigInt("0x" + hex.slice(0, 16)),
    lowBits: BigInt("0x" + hex.slice(16)),
  }
}

/**
 * A JSON-safe state schema that exercises strings, finite numbers,
 * booleans, nulls, arrays, and nested records — enough variety to
 * surface any envelope encoder bug while staying within
 * `JSON.stringify`/`JSON.parse` fidelity.
 */
const StateSchema = Schema.Struct({
  count: Schema.Int,
  label: Schema.String,
  flags: Schema.Array(Schema.Boolean),
  nested: Schema.Struct({
    name: Schema.String,
    optional: Schema.NullOr(Schema.String),
  }),
})

const stateArb = Schema.toArbitrary(StateSchema)

const dbPartArb = fc.record({
  name: fc.stringMatching(/^[a-zA-Z_][a-zA-Z0-9_]*$/),
  bytes: fc.uint8Array({ maxLength: 256 }),
})

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("snapshot-envelope properties", () => {
  it.prop(
    "JSON envelope round-trips arbitrary principal + arbitrary state",
    { principal: principalArb, state: stateArb },
    ({ principal, state }) => {
      const env = encodeJsonEnvelope(principal, state)
      expect(env.mimeType).toBe("application/json")
      const decoded = decodeEnvelope(env, { tag: "anonymous" })
      expect(decoded.kind).toBe("json")
      if (decoded.kind !== "json") throw new Error("expected json envelope")
      // Principal round-trips through serialise + deserialise.
      expect(serializePrincipal(decoded.principal)).toEqual(serializePrincipal(principal))
      // State round-trips through JSON exactly (the schema only contains
      // JSON-safe leaves).
      expect(decoded.state).toEqual(state)
    },
  )

  it.prop(
    "binary envelope round-trips arbitrary principal + arbitrary user payload",
    { principal: principalArb, userPayload: fc.uint8Array({ maxLength: 1024 }) },
    ({ principal, userPayload }) => {
      const env = encodeBinaryEnvelope(principal, userPayload)
      expect(env.mimeType).toBe("application/octet-stream")
      // The fallback principal here is anonymous; v2 envelopes carry the
      // principal inline, so the fallback must NOT be observed.
      const decoded = decodeEnvelope(env, { tag: "anonymous" })
      expect(decoded.kind).toBe("binary")
      if (decoded.kind !== "binary") throw new Error("expected binary envelope")
      expect(serializePrincipal(decoded.principal)).toEqual(serializePrincipal(principal))
      expect(Array.from(decoded.userPayload)).toEqual(Array.from(userPayload))
    },
  )

  it.prop(
    "multipart envelope round-trips arbitrary principal + state + DB parts",
    {
      principal: principalArb,
      state: stateArb,
      // De-duplicate DB names to avoid the multipart decoder's
      // duplicate-name check (an unrelated, well-tested invariant).
      databases: fc
        .uniqueArray(dbPartArb, {
          maxLength: 4,
          selector: (db) => db.name,
        })
        .map((arr) =>
          arr.map((db) => ({
            name: db.name,
            bytes: db.bytes,
          })),
        ),
    },
    ({ principal, state, databases }) => {
      const env = encodeMultipartJsonEnvelope(principal, state, databases)
      expect(env.mimeType.startsWith("multipart/mixed")).toBe(true)
      const decoded = decodeEnvelope(env, { tag: "anonymous" })
      expect(decoded.kind).toBe("multipart")
      if (decoded.kind !== "multipart") throw new Error("expected multipart envelope")
      expect(serializePrincipal(decoded.principal)).toEqual(serializePrincipal(principal))
      expect(decoded.state).toEqual(state)
      // Order is preserved by `encodeMultipartJsonEnvelope` (state first,
      // then DBs in input order); decoder strips the state part and
      // returns the DBs in the original order.
      expect(decoded.databases.length).toBe(databases.length)
      for (let i = 0; i < databases.length; i++) {
        expect(decoded.databases[i]!.name).toBe(databases[i]!.name)
        expect(Array.from(decoded.databases[i]!.bytes)).toEqual(Array.from(databases[i]!.bytes))
      }
    },
  )
})
