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
  id: { name: Schema.String },
  methods: {
    ping: method({ input: {}, success: Schema.Void }),
  },
})

const ImplementedAgent = defineAgent({
  name: "ImplementedAgent",
  id: { initial: Schema.Number },
  methods: {
    getValue: method({ input: {}, success: Schema.Number }),
  },
}).implement(({ initial }) =>
  Effect.gen(function* () {
    const ref = yield* Ref.make(initial)
    return {
      getValue: () => Ref.get(ref),
    }
  }),
)

interface RecursiveValue {
  readonly value: string
  readonly children: ReadonlyArray<RecursiveValue>
}

const recursiveSchema = (field: string): Schema.Codec<RecursiveValue, RecursiveValue> => {
  const schema: Schema.Codec<RecursiveValue, RecursiveValue> = Schema.Struct({
    value: Schema.String,
    children: Schema.Array(Schema.suspend(() => schema)),
  }).pipe(Schema.annotate({ title: field }))
  return schema
}

const RecursiveInputA = recursiveSchema("InputA")
const RecursiveOutputA = recursiveSchema("OutputA")
const RecursiveInputB = recursiveSchema("InputB")
const RecursiveOutputB = recursiveSchema("OutputB")

const RecursiveGraphsAgent = defineAgent({
  name: "RecursiveGraphsAgent",
  id: {},
  methods: {
    first: method({ input: { value: RecursiveInputA }, success: RecursiveOutputA }),
    second: method({ input: { value: RecursiveInputB }, success: RecursiveOutputB }),
  },
}).implement(() =>
  Effect.succeed({
    first: ({ value }) => Effect.succeed(value),
    second: ({ value }) => Effect.succeed(value),
  }),
)

describe("defineAgent / .implement split", () => {
  beforeEach(async () => {
    await __resetAgents()
    // Touch the agents so the test file isn't dead-code-eliminated;
    // the spec-only agent must remain importable without registering.
    void SpecOnlyAgent
    void ImplementedAgent
    void RecursiveGraphsAgent
  })

  it.effect("spec-only agents are NOT discovered by the runtime", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const names = types.map((t) => t.typeName)
      expect(names).not.toContain("SpecOnlyAgent")
    }),
  )

  it.effect("implemented agents ARE discovered by the runtime", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const names = types.map((t) => t.typeName)
      expect(names).toContain("ImplementedAgent")
    }),
  )

  it.effect("discovers distinct recursive inputs and outputs across methods", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const recursive = types.find((type) => type.typeName === "RecursiveGraphsAgent")!
      expect(recursive.schema.defs).toHaveLength(4)
      expect(new Set(recursive.schema.defs.map((definition) => definition.id)).size).toBe(4)
      for (const method of recursive.methods) {
        if (method.inputSchema.tag !== "parameters" || method.outputSchema.tag !== "single") {
          throw new Error("expected parameter input and single output")
        }
        expect(recursive.schema.typeNodes[method.inputSchema.val[0]!.schema]!.body.tag).toBe(
          "ref-type",
        )
        expect(recursive.schema.typeNodes[method.outputSchema.val]!.body.tag).toBe("ref-type")
      }
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
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    })
    const implemented = spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    expect(implemented.client).toBe(spec.client)
  })

  it("implemented agent exposes a back-reference to its spec", () => {
    const spec = defineAgent({
      name: "BackRefAgent",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    })
    const implemented = spec.implement(() => Effect.succeed({ ping: () => Effect.void }))
    expect(implemented.spec).toBe(spec)
  })

  it("mutating the original literal after defineAgent does NOT affect the spec", () => {
    const literal = {
      name: "MutationCheckAgent",
      id: { name: Schema.String },
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }
    const spec = defineAgent(literal)
    // Mutate the original literal's methods map AFTER defineAgent has
    // returned. The spec must have captured a frozen copy.
    const mutated = literal.methods as Record<string, unknown>
    mutated["sneaky"] = method({ input: {}, success: Schema.Void })
    // The spec's methods record was shallow-cloned + frozen, so the
    // post-defineAgent mutation must not be reflected.
    expect(Object.keys(spec.methods)).toEqual(["ping"])
    expect(Object.isFrozen(spec.methods)).toBe(true)
    expect(Object.isFrozen(spec.id)).toBe(true)
  })

  it("calling .implement twice on specs sharing a name surfaces as AgentError", async () => {
    // First impl: succeeds and registers.
    const specA = defineAgent({
      name: "DoubleImplAgent",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    })
    specA.implement(() => Effect.succeed({ ping: () => Effect.void }))

    // Second impl with the same name: must be stashed and re-emitted
    // from discoverAgentTypes as a typed AgentError.
    const specB = defineAgent({
      name: "DoubleImplAgent",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
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
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
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
    expect(SpecOnlyAgent.id).toBeDefined()
    expect(SpecOnlyAgent.methods).toBeDefined()
    // The flat-fields convention means there is no nested wrapper.
    expect((SpecOnlyAgent as unknown as { metadata?: unknown }).metadata).toBeUndefined()
    expect(ImplementedAgent.name).toBe("ImplementedAgent")
    expect((ImplementedAgent as unknown as { metadata?: unknown }).metadata).toBeUndefined()
  })

  it("ImplementedAgent does NOT expose .implement (type-level guard)", () => {
    const spec = defineAgent({
      name: "NoReImplementAgent",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
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
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }).implement((input) => {
      // input is the constructor-input record; no second arg
      void input
      return Effect.succeed({ ping: () => Effect.void })
    })
    void noSnap

    const withSnapSpec = defineAgent({
      name: "WithSnapAgent",
      id: {},
      snapshotting: Snapshot.define({
        schema: Schema.Struct({ count: Schema.Number }),
        policy: Snapshot.policy.default,
      }),
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    })
    const initialize = (
      input: Record<string, never>,
      snap: Parameters<Parameters<typeof withSnapSpec.implement>[0]>[1],
    ) =>
      Effect.gen(function* () {
        void input
        yield* snap.init({ count: 0 })
        return { ping: () => Effect.void }
      })
    const withSnap = withSnapSpec.implement(initialize, (_context, input, snap) =>
      initialize(input, snap),
    )
    void withSnap
    expect(true).toBe(true)
  })
})
