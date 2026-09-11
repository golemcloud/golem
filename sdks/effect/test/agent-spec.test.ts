/**
 * Tests for the `defineAgent(...).implement(...)` split — both runtime
 * semantics (spec-only agents stay out of the registry; implementations
 * register; same client reference; back-reference) and type-level
 * invariants (no `.implement` on the implemented value; snapshot/config
 * still flow through).
 */
import { describe, expect, it, beforeEach } from "@effect/vitest"
import { Effect, Ref, Schema } from "effect"
import { defineAgent, __resetAgents } from "../src/Agent.js"
import { method } from "../src/Method.js"
import { guest } from "../src/internal/guest.js"
import { Snapshot } from "../src/index.js"

const SpecOnlyAgent = defineAgent({
  name: "SpecOnlyAgent",
  constructorParams: { name: Schema.String },
  methods: {
    ping: method({ params: {}, success: Schema.Void }),
  },
})

const ImplementedAgent = defineAgent({
  name: "ImplementedAgent",
  constructorParams: { initial: Schema.Number },
  methods: {
    getValue: method({ params: {}, success: Schema.Number }),
  },
}).implement(({ initial }) =>
  Effect.gen(function* () {
    const ref = yield* Ref.make(initial)
    return {
      getValue: () => Ref.get(ref),
    }
  }),
)

describe("defineAgent / .implement split", () => {
  beforeEach(async () => {
    await __resetAgents()
    // Touch the agents so the test file isn't dead-code-eliminated;
    // the spec-only agent must remain importable without registering.
    void SpecOnlyAgent
    void ImplementedAgent
  })

  it.effect("spec-only agents are NOT discovered by the runtime", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const names = types.map((t) => t.typeName)
      expect(names).not.toContain("SpecOnlyAgent")
    }),
  )

  it.effect("implemented agents ARE discovered by the runtime", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const names = types.map((t) => t.typeName)
      expect(names).toContain("ImplementedAgent")
    }),
  )

  it("spec exposes a typed RPC client without ever being registered", () => {
    expect(typeof SpecOnlyAgent.client).toBe("object")
    expect(typeof SpecOnlyAgent.client.get).toBe("function")
    expect(typeof SpecOnlyAgent.client.getPhantom).toBe("function")
    expect(typeof SpecOnlyAgent.client.newPhantom).toBe("function")
  })

  it("implemented agent shares the SAME client reference as its spec", () => {
    const spec = defineAgent({
      name: "SharedClientAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    const implemented = spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    expect(implemented.client).toBe(spec.client)
  })

  it("implemented agent exposes a back-reference to its spec", () => {
    const spec = defineAgent({
      name: "BackRefAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    const implemented = spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    expect(implemented.spec).toBe(spec)
  })

  it("mutating the original literal after defineAgent does NOT affect the spec", () => {
    const literal = {
      name: "MutationCheckAgent",
      constructorParams: { name: Schema.String },
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    }
    const spec = defineAgent(literal)
    // Mutate the original literal's methods map AFTER defineAgent has
    // returned. The spec must have captured a frozen copy.
    const mutated = literal.methods as Record<string, unknown>
    mutated["sneaky"] = method({ params: {}, success: Schema.Void })
    // The spec's methods record was shallow-cloned + frozen, so the
    // post-defineAgent mutation must not be reflected.
    expect(Object.keys(spec.methods)).toEqual(["ping"])
    expect(Object.isFrozen(spec.methods)).toBe(true)
    expect(Object.isFrozen(spec.constructorParams)).toBe(true)
  })

  it("calling .implement twice on specs sharing a name surfaces as AgentError", async () => {
    // First impl: succeeds and registers.
    const specA = defineAgent({
      name: "DoubleImplAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    specA.implement(() => Effect.succeed({ ping: () => Effect.void }))

    // Second impl with the same name: must be stashed and re-emitted
    // from discoverAgentTypes as a typed AgentError.
    const specB = defineAgent({
      name: "DoubleImplAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    specB.implement(() => Effect.succeed({ ping: () => Effect.void }))

    let caught: unknown
    try {
      await guest.discoverAgentTypes()
    } catch (e) {
      caught = e
    }
    expect((caught as { tag?: string } | undefined)?.tag).toBe("invalid-type")
    const val = (caught as { val?: unknown } | undefined)?.val
    expect(typeof val).toBe("string")
    expect(val as string).toMatch(/DoubleImplAgent/)
  })

  it("calling .implement TWICE on the SAME spec is single-shot (deferred dup error)", async () => {
    const spec = defineAgent({
      name: "SingleShotAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    // First call: succeeds and registers.
    spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    // Second call on the same spec object: must NOT re-register; must
    // push a deferred DuplicateAgentNameError instead.
    spec.implement(() => Effect.succeed({ ping: () => Effect.void }))

    let caught: unknown
    try {
      await guest.discoverAgentTypes()
    } catch (e) {
      caught = e
    }
    expect((caught as { tag?: string } | undefined)?.tag).toBe("invalid-type")
    const val = (caught as { val?: unknown } | undefined)?.val
    expect(typeof val).toBe("string")
    expect(val as string).toMatch(/SingleShotAgent/)
  })

  it("AgentSpec exposes metadata fields FLAT (no nested .metadata)", () => {
    expect(SpecOnlyAgent.name).toBe("SpecOnlyAgent")
    expect(SpecOnlyAgent.constructorParams).toBeDefined()
    expect(SpecOnlyAgent.methods).toBeDefined()
    // The flat-fields convention means there is no nested wrapper.
    expect((SpecOnlyAgent as unknown as { metadata?: unknown }).metadata).toBeUndefined()
    expect(ImplementedAgent.name).toBe("ImplementedAgent")
    expect((ImplementedAgent as unknown as { metadata?: unknown }).metadata).toBeUndefined()
  })

  it("ImplementedAgent does NOT expose .implement (type-level guard)", () => {
    const spec = defineAgent({
      name: "NoReImplementAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    })
    const implemented = spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    // @ts-expect-error — `implement` MUST NOT exist on ImplementedAgent.
    void implemented.implement
    // `client` and `spec` MUST exist on ImplementedAgent.
    void implemented.client
    void implemented.spec
  })

  it("snap arg of impl is typed and present only when snapshot is set", () => {
    // Compile-time check: this would fail tsc if the snap arg leaked
    // into the no-snapshot variant.
    const noSnap = defineAgent({
      name: "NoSnapAgent",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    }).implement((input) => {
      // input is the constructor-input record; no second arg
      void input
      return Effect.succeed({ ping: () => Effect.void })
    })
    void noSnap

    const withSnap = defineAgent({
      name: "WithSnapAgent",
      constructorParams: {},
      snapshot: Snapshot.define({
        schema: Schema.Struct({ count: Schema.Number }),
        policy: Snapshot.policy.default,
      }),
      methods: { ping: method({ params: {}, success: Schema.Void }) },
    }).implement((input, snap) =>
      Effect.gen(function* () {
        void input
        yield* snap.init({ count: 0 })
        return { ping: () => Effect.void }
      }),
    )
    void withSnap
    expect(true).toBe(true)
  })
})
