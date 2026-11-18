import { describe, it, expect } from "@effect/vitest"
import { PrincipalRef } from "../src/InvocationContext.js"
import { callerFromHost } from "../src/internal/invocationContext.js"
import {
  accountPrincipal,
  agentPrincipal,
  anonymousPrincipal,
  toolPrincipal,
  userPrincipal,
} from "./callers.js"

describe("ordinary serialization of a live caller", () => {
  const caller = callerFromHost({
    original: userPrincipal("bob"),
    immediate: agentPrincipal(),
    tokens: [{ id: { highBits: 1n, lowBits: 2n }, maker: { issuer: "golem", subject: "account:1" } }],
  })
  const other = callerFromHost({
    original: accountPrincipal(),
    immediate: toolPrincipal(),
    tokens: [],
  })
  const anonymous = callerFromHost({
    original: anonymousPrincipal,
    immediate: anonymousPrincipal,
    tokens: [],
  })

  it("refuses a caller, each principal kind and a token provenance", () => {
    const live = [
      caller,
      caller.original,
      caller.immediate,
      caller.tokens[0],
      other.original,
      other.immediate,
      anonymous.original,
    ]
    for (const value of live) {
      expect(() => JSON.stringify(value)).toThrow(/live value.*ctx\.caller\.original\.ref/)
      expect(() => JSON.stringify({ nested: [value] })).toThrow(/live value/)
    }
  })

  it("keeps a principal reference serializable", () => {
    const ref = caller.original.ref!
    expect(ref).toBeInstanceOf(PrincipalRef)
    expect(JSON.parse(JSON.stringify(ref))).toEqual({ issuer: ref.issuer, subject: "bob" })
  })
})
