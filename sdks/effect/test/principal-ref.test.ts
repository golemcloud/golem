import { describe, it, expect } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { readFileSync } from "node:fs"
import { PrincipalRef, PrincipalRefSchema } from "../src/InvocationContext.js"
import { toWitCodec } from "../src/WitCodec.js"
import type { SchemaValue } from "../src/internal/schema-model/model.js"

const vectors = JSON.parse(
  readFileSync(
    new URL("../../../test-data/caller-identity/text-form-v1.json", import.meta.url),
    "utf8",
  ),
) as {
  readonly roundTrip: ReadonlyArray<{
    readonly name: string
    readonly issuer: string
    readonly subject: string
    readonly text: string
  }>
  readonly reject: ReadonlyArray<{ readonly name: string; readonly text: string }>
}

describe("PrincipalRef text form", () => {
  it("prints and parses exactly the shared vectors", () => {
    expect(vectors.roundTrip.length).toBeGreaterThan(0)
    for (const vector of vectors.roundTrip) {
      const ref = new PrincipalRef(vector.issuer, vector.subject)
      expect(ref.toString(), vector.name).toBe(vector.text)
      const parsed = PrincipalRef.parse(vector.text)
      expect(parsed.issuer, vector.name).toBe(vector.issuer)
      expect(parsed.subject, vector.name).toBe(vector.subject)
    }
  })

  it("refuses every non-canonical spelling", () => {
    expect(vectors.reject.length).toBeGreaterThan(0)
    for (const vector of vectors.reject) {
      expect(() => PrincipalRef.parse(vector.text), vector.name).toThrow(SyntaxError)
    }
  })

  it("compares both strings byte for byte", () => {
    const base = new PrincipalRef("https://login.example/", "Subject-1")
    expect(base.equals(new PrincipalRef("https://login.example/", "Subject-1"))).toBe(true)
    expect(base.equals(new PrincipalRef("https://login.example", "Subject-1"))).toBe(false)
    expect(base.equals(new PrincipalRef("https://login.example/", "subject-1"))).toBe(false)
    expect(base.equals(new PrincipalRef("https://login.example/", "Subject-1 "))).toBe(false)
  })
})

describe("PrincipalRefSchema", () => {
  it.effect("is the named record principal-ref { issuer, subject }", () =>
    Effect.gen(function* () {
      const codec = yield* toWitCodec(PrincipalRefSchema)
      expect(codec.graph.root.body).toEqual({ tag: "ref", id: "principal-ref" })
      const definition = codec.graph.defs.get("principal-ref")
      const body = definition?.body.body as { tag: string; fields: Array<{ name: string }> }
      expect(body.tag).toBe("record")
      expect(body.fields.map((field) => field.name)).toEqual(["issuer", "subject"])
    }),
  )

  it.effect("round-trips as a parameter value", () =>
    Effect.gen(function* () {
      const codec = yield* toWitCodec(PrincipalRefSchema)
      const original = new PrincipalRef("https://login.example", "u-1")
      const wire = yield* Schema.encodeEffect(codec.codec)(original) as Effect.Effect<SchemaValue>
      expect(wire).toMatchObject({ tag: "record" })
      const decoded = yield* Schema.decodeEffect(codec.codec)(wire) as Effect.Effect<PrincipalRef>
      expect(decoded).toBeInstanceOf(PrincipalRef)
      expect(decoded.equals(original)).toBe(true)
    }),
  )

  it("rejects a value that is not a PrincipalRef", () => {
    expect(Schema.is(PrincipalRefSchema)({ issuer: "a", subject: "b" })).toBe(false)
    expect(Schema.is(PrincipalRefSchema)(new PrincipalRef("a", "b"))).toBe(true)
  })
})
