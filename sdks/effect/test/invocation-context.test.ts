import { describe, it, expect } from "@effect/vitest"
import { Effect } from "effect"
import {
  AccountPrincipal,
  AgentPrincipal,
  AnonymousPrincipal,
  Caller,
  InvocationContext,
  PrincipalRef,
  TokenProvenance,
  ToolPrincipal,
  UserPrincipal,
} from "../src/InvocationContext.js"
import { callerFromHost, lazyInvocationContext } from "../src/internal/invocationContext.js"
import {
  accountPrincipal,
  agentPrincipal,
  anonymousPrincipal,
  toolPrincipal,
  userPrincipal,
} from "./callers.js"

const host = {
  original: userPrincipal("root"),
  immediate: agentPrincipal("worker"),
  tokens: [
    { id: { highBits: 1n, lowBits: 2n }, maker: { issuer: "golem", subject: "account:1" } },
    { id: { highBits: 3n, lowBits: 4n } },
  ],
}

describe("Caller", () => {
  it("wraps each of the five kinds of principal", () => {
    const kinds = [
      userPrincipal("alice"),
      accountPrincipal(5n),
      agentPrincipal("a"),
      toolPrincipal("t"),
      anonymousPrincipal,
    ].map((principal) => callerFromHost({ original: principal, immediate: principal, tokens: [] }))
    expect(kinds.map((caller) => caller.immediate.kind)).toEqual([
      "user",
      "account",
      "agent",
      "tool",
      "anonymous",
    ])
    expect(kinds[0]!.immediate).toBeInstanceOf(UserPrincipal)
    expect(kinds[1]!.immediate).toBeInstanceOf(AccountPrincipal)
    expect(kinds[2]!.immediate).toBeInstanceOf(AgentPrincipal)
    expect(kinds[3]!.immediate).toBeInstanceOf(ToolPrincipal)
    expect(kinds[4]!.immediate).toBeInstanceOf(AnonymousPrincipal)
  })

  it("keeps both roles and the token provenance", () => {
    const caller = callerFromHost(host)
    expect(caller.original.kind).toBe("user")
    expect(caller.immediate.kind).toBe("agent")
    expect(caller.tokens).toHaveLength(2)
    expect(caller.tokens[0]).toBeInstanceOf(TokenProvenance)
    expect(caller.tokens[0]!.maker?.toString()).toBe('("golem","account:1")')
    expect(caller.tokens[1]!.maker).toBeUndefined()
  })

  it("prints '<kind> <ref text>' and never the claims", () => {
    const caller = callerFromHost(host)
    expect(caller.original.toString()).toBe('user ("https://example.test","root")')
    expect(caller.immediate.toString()).toBe('agent ("golem","agent:worker")')
    expect(Caller.anonymous().immediate.toString()).toBe("anonymous")
  })

  it("compares a principal with a principal or a reference; anonymous equals nothing", () => {
    const caller = callerFromHost(host)
    const root = new PrincipalRef("https://example.test", "root")
    expect(caller.original.equals(root)).toBe(true)
    expect(caller.original.equals(caller.original)).toBe(true)
    expect(caller.original.equals(caller.immediate)).toBe(false)
    expect(Caller.anonymous().immediate.equals(Caller.anonymous().immediate)).toBe(false)
    expect(caller.original.equals(Caller.anonymous().immediate)).toBe(false)
  })

  it("parses the user claims lazily and reports the sign-in time", () => {
    const caller = callerFromHost({
      original: {
        tag: "user",
        val: {
          pair: { issuer: "https://example.test", subject: "alice" },
          claims: '{"email":"a@example.test"}',
          authenticatedAt: { seconds: 1_700_000_000n, nanoseconds: 500_000_000 },
        },
      },
      immediate: anonymousPrincipal,
      tokens: [],
    })
    const user = caller.original as UserPrincipal
    expect(user.claims).toEqual({ email: "a@example.test" })
    expect(user.authenticatedAt.getTime()).toBe(1_700_000_000_500)
    expect(user.issuer).toBe("https://example.test")
    expect(user.subject).toBe("alice")
  })
})

describe("InvocationContext", () => {
  it("asks the host for the caller on first use only and keeps the answer", () => {
    let reads = 0
    const context = lazyInvocationContext(() => {
      reads += 1
      return host
    })
    expect(reads).toBe(0)
    const first = context.caller
    const second = context.caller
    expect(reads).toBe(1)
    expect(second).toBe(first)
  })

  it.effect("is a service that a test can provide", () =>
    Effect.gen(function* () {
      const context = yield* InvocationContext
      expect(context.caller.immediate.kind).toBe("anonymous")
    }).pipe(Effect.provideService(InvocationContext, { caller: Caller.anonymous() })),
  )
})
