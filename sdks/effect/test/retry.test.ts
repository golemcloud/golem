import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Duration, Effect, Exit, Layer } from "effect"
import { RetryClient } from "../src/host/RetryClient.js"
import * as Retry from "../src/retry.js"
import * as RetryMock from "./mocks/golem-api-retry.js"

beforeEach(() => {
  RetryMock.__reset()
})
afterEach(() => {
  RetryMock.__reset()
})

/**
 * Default `RetryClient` Live layer used by tests that exercise the host
 * wrappers end-to-end. It binds each method to the `RetryMock` module
 * (the same module aliased to `golem:api/retry@1.5.0` in
 * `vitest.config.ts`), so test bodies can inspect / seed state via
 * `RetryMock.*` and observe the wrappers' effect on it. Replaces the
 * legacy `__setX/__resetX` indirection on `src/retry.ts`.
 */
const RetryStub = Layer.succeed(
  RetryClient,
  RetryClient.of({
    getRetryPolicies: () => RetryMock.getRetryPolicies(),
    getRetryPolicyByName: (name) => RetryMock.getRetryPolicyByName(name),
    resolveRetryPolicy: (verb, nounUri, properties) =>
      RetryMock.resolveRetryPolicy(verb, nounUri, [...properties] as Array<
        [string, RetryMock.PredicateValue]
      >),
    setRetryPolicy: (policy) => RetryMock.setRetryPolicy(policy),
    removeRetryPolicy: (name) => RetryMock.removeRetryPolicy(name),
  }),
)

describe("Retry — predicate builder", () => {
  it.effect("compiles primitive predicates to flat node arrays", () =>
    Effect.gen(function* () {
      const pred = Retry.Predicate.eq(Retry.Props.verb, "GET")
      const raw = yield* Retry.toRawPredicate(pred)
      expect(raw).toEqual({
        nodes: [
          {
            tag: "prop-eq",
            val: { propertyName: "verb", value: { tag: "text", val: "GET" } },
          },
        ],
      })
    }),
  )

  it.effect("encodes integer property values as i64 bigint", () =>
    Effect.gen(function* () {
      const pred = Retry.Predicate.gte(Retry.Props.statusCode, 500)
      const raw = yield* Retry.toRawPredicate(pred)
      expect(raw.nodes[0]).toEqual({
        tag: "prop-gte",
        val: { propertyName: "status-code", value: { tag: "integer", val: 500n } },
      })
    }),
  )

  it.effect("encodes boolean property values as boolean", () =>
    Effect.gen(function* () {
      const pred = Retry.Predicate.eq("flag", true)
      const raw = yield* Retry.toRawPredicate(pred)
      expect(raw.nodes[0]).toEqual({
        tag: "prop-eq",
        val: { propertyName: "flag", value: { tag: "boolean", val: true } },
      })
    }),
  )

  it.effect("flattens nested boolean combinators with stable child indices", () =>
    Effect.gen(function* () {
      const pred = Retry.Predicate.eq(Retry.Props.verb, "GET")
        .and(Retry.Predicate.gte(Retry.Props.statusCode, 500))
        .or(Retry.Predicate.eq(Retry.Props.statusCode, 429))
        .not()

      const raw = yield* Retry.toRawPredicate(pred)
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
    }),
  )

  it.effect("supports oneOf, exists, and pattern predicates", () =>
    Effect.gen(function* () {
      const pred = Retry.Predicate.oneOf(Retry.Props.errorType, ["timeout", "transient"])
        .and(Retry.Predicate.exists("retry-token"))
        .and(Retry.Predicate.matchesGlob(Retry.Props.uriPath, "/api/*"))
        .and(Retry.Predicate.startsWith(Retry.Props.uriHost, "api."))
        .and(Retry.Predicate.contains("user-agent", "test"))

      const raw = yield* Retry.toRawPredicate(pred)
      const tags = raw.nodes.map((n) => n.tag).sort()
      expect(tags).toContain("prop-in")
      expect(tags).toContain("prop-exists")
      expect(tags).toContain("prop-matches")
      expect(tags).toContain("prop-starts-with")
      expect(tags).toContain("prop-contains")
    }),
  )

  it.effect("rejects integer values outside i64", () =>
    Effect.gen(function* () {
      const tooBig = (1n << 64n) + 1n
      const pred = Retry.Predicate.eq("x", tooBig)
      const exit = yield* Effect.exit(Retry.toRawPredicate(pred))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        const err = exit.cause
        expect(JSON.stringify(err)).toMatch(/RetryPolicyValidationError/)
      }
    }),
  )
})

describe("Retry — policy builder", () => {
  it.effect("compiles a leaf policy", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.periodic(Duration.seconds(5))
      const raw = yield* Retry.toRawPolicy(policy)
      expect(raw).toEqual({ nodes: [{ tag: "periodic", val: 5_000_000_000n }] })
    }),
  )

  it.effect("compiles exponential / fibonacci with validated factors and durations", () =>
    Effect.gen(function* () {
      const exp = Retry.Policy.exponential(Duration.millis(250), 2)
      const expRaw = yield* Retry.toRawPolicy(exp)
      expect(expRaw.nodes[0]).toEqual({
        tag: "exponential",
        val: { baseDelay: 250_000_000n, factor: 2 },
      })

      const fib = Retry.Policy.fibonacci(Duration.millis(100), Duration.millis(150))
      const fibRaw = yield* Retry.toRawPolicy(fib)
      expect(fibRaw.nodes[0]).toEqual({
        tag: "fibonacci",
        val: { first: 100_000_000n, second: 150_000_000n },
      })
    }),
  )

  it.effect("flattens combinator chains with reserved indices", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.exponential(Duration.seconds(1), 2)
        .clamp(Duration.seconds(1), Duration.seconds(60))
        .maxRetries(5)
        .withJitter(0.25)
        .onlyWhen(Retry.Predicate.gte(Retry.Props.statusCode, 500))

      const raw = yield* Retry.toRawPolicy(policy)
      expect(raw.nodes[0].tag).toBe("filtered-on")
      if (raw.nodes[0].tag !== "filtered-on") throw new Error("unreachable")
      const inner = raw.nodes[raw.nodes[0].val.inner]
      expect(inner.tag).toBe("jitter")
    }),
  )

  it.effect("supports andThen / union / intersect", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.immediate()
        .maxRetries(3)
        .andThen(Retry.Policy.never())
        .union(Retry.Policy.periodic(Duration.seconds(1)))
        .intersect(Retry.Policy.exponential(Duration.seconds(1), 2).maxRetries(10))

      const raw = yield* Retry.toRawPolicy(policy)
      expect(raw.nodes[0].tag).toBe("policy-intersect")
    }),
  )

  it.effect("rejects clamp where minDelay > maxDelay", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.immediate().clamp(Duration.seconds(10), Duration.seconds(1))
      const exit = yield* Effect.exit(Retry.toRawPolicy(policy))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/minDelay/)
      }
    }),
  )

  it.effect("rejects non-positive exponential factor", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.exponential(Duration.seconds(1), 0)
      const exit = yield* Effect.exit(Retry.toRawPolicy(policy))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("rejects negative jitter factor", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.immediate().withJitter(-1)
      const exit = yield* Effect.exit(Retry.toRawPolicy(policy))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("rejects out-of-range maxRetries", () =>
    Effect.gen(function* () {
      const policy = Retry.Policy.immediate().maxRetries(-1)
      const exit = yield* Effect.exit(Retry.toRawPolicy(policy))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )
})

describe("Retry — NamedPolicy", () => {
  it.effect("compiles defaults: priority 0, predicate always()", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("p1", Retry.Policy.immediate())
      const raw = yield* Retry.toRawNamedPolicy(named)
      expect(raw.name).toBe("p1")
      expect(raw.priority).toBe(0)
      expect(raw.predicate.nodes[0]).toEqual({ tag: "pred-true" })
      expect(raw.policy.nodes[0]).toEqual({ tag: "immediate" })
    }),
  )

  it.effect("propagates priority and appliesWhen", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("http", Retry.Policy.periodic(Duration.seconds(1)))
        .priority(7)
        .appliesWhen(Retry.Predicate.eq(Retry.Props.verb, "GET"))
      const raw = yield* Retry.toRawNamedPolicy(named)
      expect(raw.priority).toBe(7)
      expect(raw.predicate.nodes[0]).toEqual({
        tag: "prop-eq",
        val: { propertyName: "verb", value: { tag: "text", val: "GET" } },
      })
    }),
  )

  it.effect("passes through a raw NamedRetryPolicy verbatim", () =>
    Effect.gen(function* () {
      const rawIn: RetryMock.NamedRetryPolicy = {
        name: "raw",
        priority: 3,
        predicate: { nodes: [{ tag: "pred-true" }] },
        policy: { nodes: [{ tag: "immediate" }] },
      }
      const out = yield* Retry.toRawNamedPolicy(rawIn)
      expect(out).toEqual(rawIn)
    }),
  )

  it.effect("rejects negative priority", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("bad", Retry.Policy.immediate()).priority(-1)
      const exit = yield* Effect.exit(Retry.toRawNamedPolicy(named))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )
})

describe("Retry — host wrappers (set/remove/get)", () => {
  it.effect("setPolicy persists into the host store", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("p", Retry.Policy.immediate())
      yield* Retry.setPolicy(named)
      const all = yield* Retry.getPolicies()
      expect(all.map((p) => p.name)).toEqual(["p"])
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("getPolicyByName returns undefined when unknown", () =>
    Effect.gen(function* () {
      const out = yield* Retry.getPolicyByName("missing")
      expect(out).toBeUndefined()
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("removePolicy clears the entry", () =>
    Effect.gen(function* () {
      yield* Retry.setPolicy(Retry.NamedPolicy.named("x", Retry.Policy.immediate()))
      yield* Retry.removePolicy("x")
      const out = yield* Retry.getPolicyByName("x")
      expect(out).toBeUndefined()
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("setPolicy fails with RetryPolicyValidationError on bad inputs", () =>
    Effect.gen(function* () {
      const bad = Retry.NamedPolicy.named("nope", Retry.Policy.exponential(Duration.seconds(1), -1))
      const exit = yield* Effect.exit(Retry.setPolicy(bad))
      expect(Exit.isFailure(exit)).toBe(true)
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("setPolicy wraps host throws as RetryHostError", () =>
    Effect.gen(function* () {
      const FailingSet = Layer.succeed(
        RetryClient,
        RetryClient.of({
          getRetryPolicies: () => RetryMock.getRetryPolicies(),
          getRetryPolicyByName: (name) => RetryMock.getRetryPolicyByName(name),
          resolveRetryPolicy: () => undefined,
          setRetryPolicy: () => {
            throw new Error("boom")
          },
          removeRetryPolicy: (name) => RetryMock.removeRetryPolicy(name),
        }),
      )
      const exit = yield* Effect.exit(
        Retry.setPolicy(Retry.NamedPolicy.named("p", Retry.Policy.immediate())).pipe(
          Effect.provide(FailingSet),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/RetryHostError/)
      }
    }),
  )

  it.effect("resolvePolicy encodes properties via predicate-value", () =>
    Effect.gen(function* () {
      let observed: Array<readonly [string, unknown]> | null = null
      const ObservingResolve = Layer.succeed(
        RetryClient,
        RetryClient.of({
          getRetryPolicies: () => RetryMock.getRetryPolicies(),
          getRetryPolicyByName: (name) => RetryMock.getRetryPolicyByName(name),
          resolveRetryPolicy: (_verb, _noun, props) => {
            observed = props.map(([k, v]) => [k, v])
            return undefined
          },
          setRetryPolicy: (policy) => RetryMock.setRetryPolicy(policy),
          removeRetryPolicy: (name) => RetryMock.removeRetryPolicy(name),
        }),
      )
      yield* Retry.resolvePolicy("GET", "https://example.com", [
        ["status-code", 503],
        ["verb", "GET"],
        ["debug", true],
      ]).pipe(Effect.provide(ObservingResolve))
      expect(observed).toEqual([
        ["status-code", { tag: "integer", val: 503n }],
        ["verb", { tag: "text", val: "GET" }],
        ["debug", { tag: "boolean", val: true }],
      ])
    }),
  )
})

describe("Retry — scoped helpers", () => {
  it.effect("withPolicy installs a fresh policy and removes it on exit", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("scoped", Retry.Policy.immediate())
      let snapshotInside: ReadonlyArray<{ readonly name: string }> = []
      yield* Retry.withPolicy(
        named,
        Effect.sync(() => {
          snapshotInside = RetryMock.getRetryPolicies()
        }),
      )
      expect(snapshotInside.map((p) => p.name)).toEqual(["scoped"])
      expect(RetryMock.getRetryPolicies().map((p) => p.name)).toEqual([])
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("withPolicy restores a previously-existing policy on exit", () =>
    Effect.gen(function* () {
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
      yield* Retry.withPolicy(
        overlay,
        Effect.sync(() => {
          insidePolicy = RetryMock.getRetryPolicyByName("scoped")?.policy
        }),
      )
      expect(insidePolicy?.nodes[0]?.tag).toBe("exponential")
      // After the scope closes, the original policy is restored.
      const after = RetryMock.getRetryPolicyByName("scoped")
      expect(after?.policy.nodes[0]?.tag).toBe("immediate")
    }).pipe(Effect.provide(RetryStub)),
  )

  it.effect("withPolicy restores on failure too", () =>
    Effect.gen(function* () {
      const named = Retry.NamedPolicy.named("scoped", Retry.Policy.immediate())
      const exit = yield* Effect.exit(Retry.withPolicy(named, Effect.fail("boom" as const)))
      expect(Exit.isFailure(exit)).toBe(true)
      expect(RetryMock.getRetryPolicies()).toEqual([])
    }).pipe(Effect.provide(RetryStub)),
  )
})
