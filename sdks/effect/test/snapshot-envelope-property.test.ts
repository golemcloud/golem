import { describe, expect } from "@effect/vitest"
import { Schema } from "effect"
import * as fc from "fast-check"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  encodeMultipartJsonEnvelope,
} from "../src/internal/snapshotEnvelope.js"
import { prop } from "./property.js"

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

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

const stateArb: fc.Arbitrary<typeof StateSchema.Type> = fc.record({
  count: fc.integer(),
  label: fc.string(),
  flags: fc.array(fc.boolean()),
  nested: fc.record({
    name: fc.string(),
    optional: fc.oneof(fc.constant(null), fc.string()),
  }),
})

const dbPartArb = fc.record({
  name: fc.stringMatching(/^[a-zA-Z_][a-zA-Z0-9_]*$/),
  bytes: fc.uint8Array({ maxLength: 256 }),
})

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("snapshot-envelope properties", () => {
  prop(
    "JSON envelope round-trips arbitrary state without a principal",
    { state: stateArb },
    ({ state }) => {
      expect(Schema.is(StateSchema)(state)).toBe(true)
      const env = encodeJsonEnvelope(state)
      expect(env.mimeType).toBe("application/json")
      const decoded = decodeEnvelope(env)
      expect(decoded.kind).toBe("json")
      if (decoded.kind !== "json") throw new Error("expected json envelope")
      expect(decoded).not.toHaveProperty("principal")
      // State round-trips through JSON exactly (the schema only contains
      // JSON-safe leaves).
      expect(decoded.state).toEqual(state)
    },
  )

  prop(
    "binary envelope round-trips an arbitrary user payload",
    { userPayload: fc.uint8Array({ maxLength: 1024 }) },
    ({ userPayload }) => {
      const env = encodeBinaryEnvelope(userPayload)
      expect(env.mimeType).toBe("application/octet-stream")
      const decoded = decodeEnvelope(env)
      expect(decoded.kind).toBe("binary")
      if (decoded.kind !== "binary") throw new Error("expected binary envelope")
      expect(Array.from(decoded.userPayload)).toEqual(Array.from(userPayload))
    },
  )

  prop(
    "multipart envelope round-trips arbitrary state + DB parts",
    {
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
    ({ state, databases }) => {
      const env = encodeMultipartJsonEnvelope(state, databases)
      expect(env.mimeType.startsWith("multipart/mixed")).toBe(true)
      const decoded = decodeEnvelope(env)
      expect(decoded.kind).toBe("multipart")
      if (decoded.kind !== "multipart") throw new Error("expected multipart envelope")
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
