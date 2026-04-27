import { Duration, Effect, Exit } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Retry from "../src/retry.js"
import * as RetryMock from "./mocks/golem-api-retry.js"

const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)
const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)

beforeEach(() => {
  RetryMock.__reset()
})
afterEach(() => {
  RetryMock.__reset()
})

describe("Retry — predicate builder", () => {
  it("compiles primitive predicates to flat node arrays", async () => {
    const pred = Retry.Predicate.eq(Retry.Props.verb, "GET")
    const raw = await runP(Retry.toRawPredicate(pred))
    expect(raw).toEqual({
      nodes: [
        {
          tag: "prop-eq",
          val: { propertyName: "verb", value: { tag: "text", val: "GET" } },
        },
      ],
    })
  })

  it("encodes integer property values as i64 bigint", async () => {
    const pred = Retry.Predicate.gte(Retry.Props.statusCode, 500)
    const raw = await runP(Retry.toRawPredicate(pred))
    expect(raw.nodes[0]).toEqual({
      tag: "prop-gte",
      val: { propertyName: "status-code", value: { tag: "integer", val: 500n } },
    })
  })

  it("encodes boolean property values as boolean", async () => {
    const pred = Retry.Predicate.eq("flag", true)
    const raw = await runP(Retry.toRawPredicate(pred))
    expect(raw.nodes[0]).toEqual({
      tag: "prop-eq",
      val: { propertyName: "flag", value: { tag: "boolean", val: true } },
    })
  })

  it("flattens nested boolean combinators with stable child indices", async () => {
    const pred = Retry.Predicate.eq(Retry.Props.verb, "GET")
      .and(Retry.Predicate.gte(Retry.Props.statusCode, 500))
      .or(Retry.Predicate.eq(Retry.Props.statusCode, 429))
      .not()

    const raw = await runP(Retry.toRawPredicate(pred))
    // Root must be at index 0.
    const root = raw.nodes[0]
    expect(root.tag).toBe("pred-not")
    if (root.tag !== "pred-not") throw new Error("unreachable")
    const orNode = raw.nodes[root.val]
    expect(orNode.tag).toBe("pred-or")
    if (orNode.tag !== "pred-or") throw new Error("unreachable")
    const [andIdx, eq429Idx] = orNode.val
    expect(raw.nodes[andIdx].tag).toBe("pred-and")
    expect(raw.nodes[eq429Idx]).toEqual({
      tag: "prop-eq",
      val: { propertyName: "status-code", value: { tag: "integer", val: 429n } },
    })
  })

  it("supports oneOf, exists, and pattern predicates", async () => {
    const pred = Retry.Predicate.oneOf(Retry.Props.errorType, ["timeout", "transient"])
      .and(Retry.Predicate.exists("retry-token"))
      .and(Retry.Predicate.matchesGlob(Retry.Props.uriPath, "/api/*"))
      .and(Retry.Predicate.startsWith(Retry.Props.uriHost, "api."))
      .and(Retry.Predicate.contains("user-agent", "test"))

    const raw = await runP(Retry.toRawPredicate(pred))
    const tags = raw.nodes.map((n) => n.tag).sort()
    expect(tags).toContain("prop-in")
    expect(tags).toContain("prop-exists")
    expect(tags).toContain("prop-matches")
    expect(tags).toContain("prop-starts-with")
    expect(tags).toContain("prop-contains")
  })

  it("rejects integer values outside i64", async () => {
    const tooBig = (1n << 64n) + 1n
    const pred = Retry.Predicate.eq("x", tooBig)
    const exit = await runExit(Retry.toRawPredicate(pred))
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      const err = exit.cause
      expect(JSON.stringify(err)).toMatch(/RetryPolicyValidationError/)
    }
  })
})

describe("Retry — policy builder", () => {
  it("compiles a leaf policy", async () => {
    const policy = Retry.Policy.periodic(Duration.seconds(5))
    const raw = await runP(Retry.toRawPolicy(policy))
    expect(raw).toEqual({ nodes: [{ tag: "periodic", val: 5_000_000_000n }] })
  })

  it("compiles exponential / fibonacci with validated factors and durations", async () => {
    const exp = Retry.Policy.exponential(Duration.millis(250), 2)
    const expRaw = await runP(Retry.toRawPolicy(exp))
    expect(expRaw.nodes[0]).toEqual({
      tag: "exponential",
      val: { baseDelay: 250_000_000n, factor: 2 },
    })

    const fib = Retry.Policy.fibonacci(Duration.millis(100), Duration.millis(150))
    const fibRaw = await runP(Retry.toRawPolicy(fib))
    expect(fibRaw.nodes[0]).toEqual({
      tag: "fibonacci",
      val: { first: 100_000_000n, second: 150_000_000n },
    })
  })

  it("flattens combinator chains with reserved indices", async () => {
    const policy = Retry.Policy.exponential(Duration.seconds(1), 2)
      .clamp(Duration.seconds(1), Duration.seconds(60))
      .maxRetries(5)
      .withJitter(0.25)
      .onlyWhen(Retry.Predicate.gte(Retry.Props.statusCode, 500))

    const raw = await runP(Retry.toRawPolicy(policy))
    expect(raw.nodes[0].tag).toBe("filtered-on")
    if (raw.nodes[0].tag !== "filtered-on") throw new Error("unreachable")
    const inner = raw.nodes[raw.nodes[0].val.inner]
    expect(inner.tag).toBe("jitter")
  })

  it("supports andThen / union / intersect", async () => {
    const policy = Retry.Policy.immediate()
      .maxRetries(3)
      .andThen(Retry.Policy.never())
      .union(Retry.Policy.periodic(Duration.seconds(1)))
      .intersect(Retry.Policy.exponential(Duration.seconds(1), 2).maxRetries(10))

    const raw = await runP(Retry.toRawPolicy(policy))
    expect(raw.nodes[0].tag).toBe("policy-intersect")
  })

  it("rejects clamp where minDelay > maxDelay", async () => {
    const policy = Retry.Policy.immediate().clamp(Duration.seconds(10), Duration.seconds(1))
    const exit = await runExit(Retry.toRawPolicy(policy))
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      expect(JSON.stringify(exit.cause)).toMatch(/minDelay/)
    }
  })

  it("rejects non-positive exponential factor", async () => {
    const policy = Retry.Policy.exponential(Duration.seconds(1), 0)
    const exit = await runExit(Retry.toRawPolicy(policy))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("rejects negative jitter factor", async () => {
    const policy = Retry.Policy.immediate().withJitter(-1)
    const exit = await runExit(Retry.toRawPolicy(policy))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("rejects out-of-range maxRetries", async () => {
    const policy = Retry.Policy.immediate().maxRetries(-1)
    const exit = await runExit(Retry.toRawPolicy(policy))
    expect(Exit.isFailure(exit)).toBe(true)
  })
})

describe("Retry — NamedPolicy", () => {
  it("compiles defaults: priority 0, predicate always()", async () => {
    const named = Retry.NamedPolicy.named("p1", Retry.Policy.immediate())
    const raw = await runP(Retry.toRawNamedPolicy(named))
    expect(raw.name).toBe("p1")
    expect(raw.priority).toBe(0)
    expect(raw.predicate.nodes[0]).toEqual({ tag: "pred-true" })
    expect(raw.policy.nodes[0]).toEqual({ tag: "immediate" })
  })

  it("propagates priority and appliesWhen", async () => {
    const named = Retry.NamedPolicy.named("http", Retry.Policy.periodic(Duration.seconds(1)))
      .priority(7)
      .appliesWhen(Retry.Predicate.eq(Retry.Props.verb, "GET"))
    const raw = await runP(Retry.toRawNamedPolicy(named))
    expect(raw.priority).toBe(7)
    expect(raw.predicate.nodes[0]).toEqual({
      tag: "prop-eq",
      val: { propertyName: "verb", value: { tag: "text", val: "GET" } },
    })
  })

  it("passes through a raw NamedRetryPolicy verbatim", async () => {
    const rawIn: RetryMock.NamedRetryPolicy = {
      name: "raw",
      priority: 3,
      predicate: { nodes: [{ tag: "pred-true" }] },
      policy: { nodes: [{ tag: "immediate" }] },
    }
    const out = await runP(Retry.toRawNamedPolicy(rawIn))
    expect(out).toEqual(rawIn)
  })

  it("rejects negative priority", async () => {
    const named = Retry.NamedPolicy.named("bad", Retry.Policy.immediate()).priority(-1)
    const exit = await runExit(Retry.toRawNamedPolicy(named))
    expect(Exit.isFailure(exit)).toBe(true)
  })
})

describe("Retry — host wrappers (set/remove/get)", () => {
  it("setPolicy persists into the host store", async () => {
    const named = Retry.NamedPolicy.named("p", Retry.Policy.immediate())
    await runP(Retry.setPolicy(named))
    const all = await runP(Retry.getPolicies())
    expect(all.map((p) => p.name)).toEqual(["p"])
  })

  it("getPolicyByName returns undefined when unknown", async () => {
    const out = await runP(Retry.getPolicyByName("missing"))
    expect(out).toBeUndefined()
  })

  it("removePolicy clears the entry", async () => {
    await runP(Retry.setPolicy(Retry.NamedPolicy.named("x", Retry.Policy.immediate())))
    await runP(Retry.removePolicy("x"))
    const out = await runP(Retry.getPolicyByName("x"))
    expect(out).toBeUndefined()
  })

  it("setPolicy fails with RetryPolicyValidationError on bad inputs", async () => {
    const bad = Retry.NamedPolicy.named("nope", Retry.Policy.exponential(Duration.seconds(1), -1))
    const exit = await runExit(Retry.setPolicy(bad))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("setPolicy wraps host throws as RetryHostError", async () => {
    Retry.__setSetRetryPolicyForTest(() => {
      throw new Error("boom")
    })
    try {
      const exit = await runExit(
        Retry.setPolicy(Retry.NamedPolicy.named("p", Retry.Policy.immediate())),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/RetryHostError/)
      }
    } finally {
      Retry.__resetSetRetryPolicyForTest()
    }
  })

  it("resolvePolicy encodes properties via predicate-value", async () => {
    let observed: Array<readonly [string, unknown]> | null = null
    Retry.__setResolveRetryPolicyForTest((_v, _n, props) => {
      observed = props.map(([k, v]) => [k, v])
      return undefined
    })
    try {
      await runP(
        Retry.resolvePolicy("GET", "https://example.com", [
          ["status-code", 503],
          ["verb", "GET"],
          ["debug", true],
        ]),
      )
      expect(observed).toEqual([
        ["status-code", { tag: "integer", val: 503n }],
        ["verb", { tag: "text", val: "GET" }],
        ["debug", { tag: "boolean", val: true }],
      ])
    } finally {
      Retry.__resetResolveRetryPolicyForTest()
    }
  })
})

describe("Retry — scoped helpers", () => {
  it("withPolicy installs a fresh policy and removes it on exit", async () => {
    const named = Retry.NamedPolicy.named("scoped", Retry.Policy.immediate())
    let snapshotInside: ReadonlyArray<{ readonly name: string }> = []
    await runP(
      Retry.withPolicy(
        named,
        Effect.sync(() => {
          snapshotInside = RetryMock.getRetryPolicies()
        }),
      ),
    )
    expect(snapshotInside.map((p) => p.name)).toEqual(["scoped"])
    expect(RetryMock.getRetryPolicies().map((p) => p.name)).toEqual([])
  })

  it("withPolicy restores a previously-existing policy on exit", async () => {
    // Pre-existing policy under the same name.
    RetryMock.setRetryPolicy({
      name: "scoped",
      priority: 0,
      predicate: { nodes: [{ tag: "pred-true" }] },
      policy: { nodes: [{ tag: "immediate" }] },
    })

    const overlay = Retry.NamedPolicy.named(
      "scoped",
      Retry.Policy.exponential(Duration.seconds(1), 2),
    )
    let insidePolicy: { readonly nodes: ReadonlyArray<{ readonly tag: string }> } | undefined
    await runP(
      Retry.withPolicy(
        overlay,
        Effect.sync(() => {
          insidePolicy = RetryMock.getRetryPolicyByName("scoped")?.policy
        }),
      ),
    )
    expect(insidePolicy?.nodes[0]?.tag).toBe("exponential")
    // After the scope closes, the original policy is restored.
    const after = RetryMock.getRetryPolicyByName("scoped")
    expect(after?.policy.nodes[0]?.tag).toBe("immediate")
  })

  it("withPolicy restores on failure too", async () => {
    const named = Retry.NamedPolicy.named("scoped", Retry.Policy.immediate())
    const exit = await runExit(Retry.withPolicy(named, Effect.fail("boom" as const)))
    expect(Exit.isFailure(exit)).toBe(true)
    expect(RetryMock.getRetryPolicies()).toEqual([])
  })
})
