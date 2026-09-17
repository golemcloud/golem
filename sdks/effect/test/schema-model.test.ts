import { describe, expect, it } from "vitest"
import type { Secret as RawSecret } from "golem:core/types@2.0.0"
import { emptyMetadata, schemaType, t, v } from "../src/internal/schema-model/model.js"
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
} from "../src/internal/schema-model/wit.js"
import { validateSchemaGraph } from "../src/internal/schema-model/validation.js"
import { withCapabilityAdoptionTransaction } from "../src/internal/schema-model/capabilityTransaction.js"
import {
  adoptGuestSecretHandle,
  createGuestSecretHandle,
} from "../src/internal/schema-model/secretHandle.js"
import { SECRET_INTERNAL } from "../src/internal/schema-model/secretInternal.js"

describe("schema-native graph and value model", () => {
  it("round-trips recursive definitions and rich nodes through canonical WIT", () => {
    const metadata = emptyMetadata()
    const node = t.record([
      {
        name: "name",
        body: schemaType({ tag: "text", restrictions: { minLength: 1 } }),
        metadata,
      },
      { name: "children", body: t.list(t.ref("example.node")), metadata },
      { name: "created", body: t.datetime(), metadata },
    ])
    const graph = {
      defs: new Map([["example.node", { name: "Node", body: node }]]),
      root: t.ref("example.node"),
    }
    expect(validateSchemaGraph(graph)).toEqual([])
    expect(schemaGraphFromWit(schemaGraphToWit(graph))).toEqual(graph)

    const value = v.record([
      v.text("root", "en"),
      v.list([]),
      v.datetime({ seconds: 1n, nanoseconds: 2 }),
    ])
    expect(schemaValueFromWit(schemaValueToWit(value))).toEqual(value)
  })

  it("rejects duplicate affine handles before moving either occurrence", () => {
    const raw = {} as RawSecret
    const handle = createGuestSecretHandle(SECRET_INTERNAL, raw)
    expect(() => schemaValueToWit(v.list([v.secret(handle), v.secret(handle)]))).toThrow(
      /more than once/,
    )
    expect(schemaValueToWit(v.secret(handle)).valueNodes[0]?.tag).toBe("secret-value")
  })

  it("rolls back adopted capabilities when a composite conversion fails", () => {
    const raw = {} as RawSecret
    expect(() =>
      withCapabilityAdoptionTransaction((transaction) => {
        adoptGuestSecretHandle(SECRET_INTERNAL, raw, transaction)
        throw new Error("later field failed")
      }),
    ).toThrow("later field failed")
    expect(createGuestSecretHandle(SECRET_INTERNAL, raw)).toBeDefined()
  })
})
