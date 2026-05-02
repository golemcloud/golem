import { describe, expect, it } from "@effect/vitest"
import { Cause, Duration, Effect, Pull, Random, Ref, Result, Schedule } from "effect"
import * as Retry from "../src/Retry.js"

/**
 * Drive a schedule deterministically, collecting every emitted
 * `[output, delay]` pair (and noting if/why it terminated). The fake
 * `now` clock advances in lockstep with the schedule's reported delays
 * so a `spaced(d)` schedule sees consistent input timing.
 */
const collect = <Out, In, E, R>(
  schedule: Schedule.Schedule<Out, In, E, R>,
  inputs: ReadonlyArray<In>,
): Effect.Effect<
  {
    decisions: Array<{ output: Out; delayMillis: number }>
    done: boolean
    doneOutput?: Out
  },
  E,
  R
> =>
  Effect.gen(function* () {
    const step = yield* Schedule.toStep(schedule)
    let now = 0
    const decisions: Array<{ output: Out; delayMillis: number }> = []
    let done = false
    let doneOutput: Out | undefined
    for (const input of inputs) {
      const result = yield* Pull.matchEffect(step(now, input), {
        onSuccess: (r) => Effect.succeed({ kind: "next" as const, result: r }),
        onFailure: Effect.failCause,
        onDone: (out) => Effect.succeed({ kind: "done" as const, out }),
      })
      if (result.kind === "done") {
        done = true
        doneOutput = result.out
        break
      }
      const [output, delay] = result.result
      const delayMillis = Duration.toMillis(delay)
      decisions.push({ output, delayMillis })
      now += delayMillis
    }
    return { decisions, done, doneOutput }
  })

describe("Retry.toSchedule — leaf strategies", () => {
  it.effect("immediate emits zero-delay decisions forever", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(Retry.Policy.immediate())
      const { decisions, done } = yield* collect(schedule, [null, null, null, null, null])
      expect(done).toBe(false)
      expect(decisions).toHaveLength(5)
      for (const d of decisions) expect(d.delayMillis).toBe(0)
    }),
  )

  it.effect("never terminates on the first decision", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(Retry.Policy.never())
      const { decisions, done } = yield* collect(schedule, [null, null, null])
      expect(done).toBe(true)
      expect(decisions).toHaveLength(0)
    }),
  )

  it.effect("periodic spaces decisions by the configured delay", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(Retry.Policy.periodic(Duration.millis(250)))
      const { decisions, done } = yield* collect(schedule, [null, null, null, null])
      expect(done).toBe(false)
      expect(decisions.map((d) => d.delayMillis)).toEqual([250, 250, 250, 250])
    }),
  )

  it.effect("exponential grows by the configured factor", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(Retry.Policy.exponential(Duration.millis(100), 2))
      const { decisions } = yield* collect(schedule, [null, null, null, null, null])
      expect(decisions.map((d) => d.delayMillis)).toEqual([100, 200, 400, 800, 1600])
    }),
  )

  it.effect("fibonacci honours both seeds", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.fibonacci(Duration.millis(100), Duration.millis(150)),
      )
      const { decisions } = yield* collect(schedule, [null, null, null, null, null, null])
      // first=100, second=150, then a+b sequence.
      expect(decisions.map((d) => d.delayMillis)).toEqual([100, 150, 250, 400, 650, 1050])
    }),
  )
})

describe("Retry.toSchedule — combinators", () => {
  it.effect("count-box (maxRetries) caps the number of recurrences", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(Retry.Policy.immediate().maxRetries(3))
      const { decisions, done } = yield* collect(schedule, [null, null, null, null, null, null])
      // recurs(3) yields attempt <= 3 → 3 decisions, then done.
      expect(decisions).toHaveLength(3)
      expect(done).toBe(true)
    }),
  )

  it.effect("clamp-delay clamps the inner delay into [min, max]", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.exponential(Duration.millis(50), 4).clamp(
          Duration.millis(100),
          Duration.millis(500),
        ),
      )
      const { decisions } = yield* collect(schedule, [null, null, null, null, null])
      // Raw exponential: 50, 200, 800, 3200, 12800.
      // After clamp(100, 500): 100, 200, 500, 500, 500.
      expect(decisions.map((d) => d.delayMillis)).toEqual([100, 200, 500, 500, 500])
    }),
  )

  it.effect("add-delay adds a fixed delay on top of the inner output", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.periodic(Duration.millis(100)).addDelay(Duration.millis(25)),
      )
      const { decisions } = yield* collect(schedule, [null, null, null])
      expect(decisions.map((d) => d.delayMillis)).toEqual([125, 125, 125])
    }),
  )

  it.effect("jitter scales each delay into [d·(1-f), d·(1+f)]", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.periodic(Duration.millis(100)).withJitter(0.2),
      )
      const { decisions } = yield* collect(
        schedule,
        Array.from({ length: 100 }, () => null),
      )
      for (const d of decisions) {
        expect(d.delayMillis).toBeGreaterThanOrEqual(80)
        expect(d.delayMillis).toBeLessThanOrEqual(120)
      }
      // Verify we actually saw variation (deterministic via the test
      // Random — values will differ across attempts).
      const min = Math.min(...decisions.map((d) => d.delayMillis))
      const max = Math.max(...decisions.map((d) => d.delayMillis))
      expect(max - min).toBeGreaterThan(0)
    }).pipe(Random.withSeed("retry-schedule-jitter-test")),
  )

  it.effect("and-then runs left to completion, then right", () =>
    Effect.gen(function* () {
      const left = Retry.Policy.periodic(Duration.millis(50)).maxRetries(2)
      const right = Retry.Policy.periodic(Duration.millis(200)).maxRetries(2)
      const schedule = yield* Retry.toSchedule(left.andThen(right))
      const { decisions, done } = yield* collect(schedule, [null, null, null, null, null, null])
      expect(decisions.map((d) => d.delayMillis)).toEqual([50, 50, 200, 200])
      expect(done).toBe(true)
    }),
  )

  it.effect("policy-intersect uses the MAX delay and stops with the more restrictive", () =>
    Effect.gen(function* () {
      const left = Retry.Policy.periodic(Duration.millis(100)).maxRetries(2)
      const right = Retry.Policy.periodic(Duration.millis(50)).maxRetries(5)
      const schedule = yield* Retry.toSchedule(left.intersect(right))
      const { decisions, done } = yield* collect(schedule, [null, null, null, null, null])
      // Both must continue. Left runs out after 2 decisions → schedule done.
      // Each delay = MAX(100, 50) = 100.
      expect(decisions).toHaveLength(2)
      expect(decisions.map((d) => d.delayMillis)).toEqual([100, 100])
      expect(done).toBe(true)
    }),
  )

  it.effect("policy-union uses the MIN delay and stops only when both are done", () =>
    Effect.gen(function* () {
      const left = Retry.Policy.periodic(Duration.millis(100)).maxRetries(2)
      const right = Retry.Policy.periodic(Duration.millis(50)).maxRetries(5)
      const schedule = yield* Retry.toSchedule(left.union(right))
      const { decisions, done } = yield* collect(schedule, [null, null, null, null, null, null])
      // Continues while either wants. Min(100, 50) = 50 throughout.
      expect(decisions).toHaveLength(5)
      expect(decisions.map((d) => d.delayMillis)).toEqual([50, 50, 50, 50, 50])
      expect(done).toBe(true)
    }),
  )

  it.effect("filtered-on stops when the predicate rejects the input", () =>
    Effect.gen(function* () {
      type Err = { code: number }
      const policy = Retry.Policy.immediate().onlyWhen(
        Retry.Predicate.gte(Retry.Props.statusCode, 500),
      )
      const schedule = yield* Retry.toSchedule<Err>(policy, {
        properties: (e) => ({ [Retry.Props.statusCode]: e.code }),
      })
      const inputs: Array<Err> = [
        { code: 503 },
        { code: 502 },
        { code: 404 }, // < 500, predicate false → stop
        { code: 500 },
      ]
      const { decisions, done } = yield* collect(schedule, inputs)
      expect(decisions).toHaveLength(2)
      expect(done).toBe(true)
    }),
  )

  it.effect(
    "filtered-on inside union: branch is skipped per-input (re-enables on later true input)",
    () =>
      Effect.gen(function* () {
        type Err = { code: number }
        const filtered = Retry.Policy.periodic(Duration.millis(50)).onlyWhen(
          Retry.Predicate.gte(Retry.Props.statusCode, 500),
        )
        const fallback = Retry.Policy.periodic(Duration.millis(200))
        const schedule = yield* Retry.toSchedule<Err>(filtered.union(fallback), {
          properties: (e) => ({ [Retry.Props.statusCode]: e.code }),
        })
        const inputs: Array<Err> = [
          { code: 500 }, // filtered active → MIN(50, 200) = 50
          { code: 404 }, // filtered skipped → fallback alone = 200
          { code: 500 }, // filtered active again → MIN(50, 200) = 50
          { code: 500 }, // filtered active → 50
        ]
        const { decisions } = yield* collect(schedule, inputs)
        // Effect's `Schedule.while` is per-step, not permanent — when the
        // predicate is false for one input the branch yields `Cause.done`
        // for that step, and `Schedule.either` emits only the other side's
        // delay. On the next input the predicate is re-evaluated; if true
        // the branch participates again. This matches host semantics for
        // `filtered-on` inside `policy-union` more closely than a
        // permanent extinguish would.
        expect(decisions.map((d) => d.delayMillis)).toEqual([50, 200, 50, 50])
      }),
  )

  it.effect("time-box may overshoot when inner's next delay exceeds the remaining budget", () =>
    Effect.gen(function* () {
      // Inner asks for 1000ms periodic; budget is 500ms. The first
      // decision still emits a 1000ms delay because elapsed (0) is
      // within the box; only the SECOND decision is rejected (elapsed
      // 1000 > 500). Documented as an approximation.
      const policy = Retry.Policy.periodic(Duration.seconds(1)).within(Duration.millis(500))
      const schedule = yield* Retry.toSchedule(policy)
      const { decisions, done } = yield* collect(schedule, [null, null, null])
      expect(decisions).toHaveLength(1)
      expect(decisions[0].delayMillis).toBe(1000)
      expect(done).toBe(true)
    }),
  )

  it.effect("jitter with factor 0 leaves the delay unchanged", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.periodic(Duration.millis(120)).withJitter(0),
      )
      const { decisions } = yield* collect(schedule, [null, null, null, null])
      for (const d of decisions) expect(d.delayMillis).toBe(120)
    }),
  )

  it.effect("jitter with factor > 1 stays in [0, 1+f) (clamp-at-zero approximation)", () =>
    Effect.gen(function* () {
      const schedule = yield* Retry.toSchedule(
        Retry.Policy.periodic(Duration.millis(100)).withJitter(2),
      )
      const { decisions } = yield* collect(
        schedule,
        Array.from({ length: 200 }, () => null),
      )
      for (const d of decisions) {
        expect(d.delayMillis).toBeGreaterThanOrEqual(0)
        // factor=2 → upper bound (1+f)·d = 3·100 = 300.
        expect(d.delayMillis).toBeLessThanOrEqual(300)
      }
    }).pipe(Random.withSeed("retry-schedule-jitter-overscale-test")),
  )
})

describe("Retry.toSchedule — host integration", () => {
  it.effect("accepts a raw RetryPolicy returned by the host", () =>
    Effect.gen(function* () {
      // Build a raw policy by going through the public encoder, then feed
      // the raw form back into toSchedule (simulating a host fetch).
      const raw = yield* Retry.toRawPolicy(Retry.Policy.periodic(Duration.millis(75)))
      const schedule = yield* Retry.toSchedule(raw)
      const { decisions } = yield* collect(schedule, [null, null, null])
      expect(decisions.map((d) => d.delayMillis)).toEqual([75, 75, 75])
    }),
  )

  it.effect("Effect.retry with the produced schedule retries the right number of times", () =>
    Effect.gen(function* () {
      const calls = yield* Ref.make(0)
      const failing = Effect.gen(function* () {
        const n = yield* Ref.updateAndGet(calls, (c) => c + 1)
        if (n < 4) return yield* Effect.fail(new Error(`attempt ${n}`))
        return n
      })
      const schedule = yield* Retry.toSchedule(Retry.Policy.immediate().maxRetries(5))
      const result = yield* failing.pipe(Effect.retry(schedule))
      expect(result).toBe(4)
      expect(yield* Ref.get(calls)).toBe(4)
    }),
  )

  it.effect("Effect.retry stops once the schedule is exhausted", () =>
    Effect.gen(function* () {
      const calls = yield* Ref.make(0)
      const failing = Effect.gen(function* () {
        yield* Ref.update(calls, (c) => c + 1)
        return yield* Effect.fail(new Error("nope"))
      })
      const schedule = yield* Retry.toSchedule(Retry.Policy.immediate().maxRetries(2))
      const exit = yield* Effect.exit(failing.pipe(Effect.retry(schedule)))
      expect(exit._tag).toBe("Failure")
      // 1 initial attempt + 2 retries = 3 calls.
      expect(yield* Ref.get(calls)).toBe(3)
    }),
  )

  it.effect("propagates RetryPolicyValidationError when the policy is malformed", () =>
    Effect.gen(function* () {
      // Negative jitter factor → validation failure.
      const policy = Retry.Policy.immediate().withJitter(-1)
      const exit = yield* Effect.exit(Retry.toSchedule(policy))
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const found = Cause.findError(exit.cause)
        expect(Result.isSuccess(found)).toBe(true)
        if (Result.isSuccess(found)) {
          expect(found.success).toBeInstanceOf(Retry.RetryPolicyValidationError)
        }
      }
    }),
  )
})

describe("Retry.evaluatePredicate", () => {
  it("evaluates equality, inequality and ordering on integers", () =>
    Effect.gen(function* () {
      const pred = yield* Retry.toRawPredicate(
        Retry.Predicate.gte(Retry.Props.statusCode, 500).and(
          Retry.Predicate.neq(Retry.Props.statusCode, 501),
        ),
      )
      expect(Retry.evaluatePredicate(pred, { [Retry.Props.statusCode]: 500 })).toBe(true)
      expect(Retry.evaluatePredicate(pred, { [Retry.Props.statusCode]: 503 })).toBe(true)
      expect(Retry.evaluatePredicate(pred, { [Retry.Props.statusCode]: 501 })).toBe(false)
      expect(Retry.evaluatePredicate(pred, { [Retry.Props.statusCode]: 404 })).toBe(false)
      expect(Retry.evaluatePredicate(pred, {})).toBe(false)
    }).pipe(Effect.runSync))

  it("evaluates oneOf, exists, and string ops", () =>
    Effect.gen(function* () {
      const pred = yield* Retry.toRawPredicate(
        Retry.Predicate.oneOf(Retry.Props.verb, ["GET", "HEAD"])
          .and(Retry.Predicate.exists(Retry.Props.uriHost))
          .and(Retry.Predicate.startsWith(Retry.Props.uriPath, "/api/")),
      )
      expect(
        Retry.evaluatePredicate(pred, {
          verb: "GET",
          "uri-host": "example.com",
          "uri-path": "/api/v1/users",
        }),
      ).toBe(true)
      expect(
        Retry.evaluatePredicate(pred, {
          verb: "POST",
          "uri-host": "example.com",
          "uri-path": "/api/v1/users",
        }),
      ).toBe(false)
      expect(
        Retry.evaluatePredicate(pred, {
          verb: "GET",
          "uri-path": "/api/v1/users",
        }),
      ).toBe(false)
      expect(
        Retry.evaluatePredicate(pred, {
          verb: "GET",
          "uri-host": "example.com",
          "uri-path": "/static/asset",
        }),
      ).toBe(false)
    }).pipe(Effect.runSync))

  it("evaluates pred-true/pred-false/not", () =>
    Effect.gen(function* () {
      const t = yield* Retry.toRawPredicate(Retry.Predicate.always())
      const f = yield* Retry.toRawPredicate(Retry.Predicate.never())
      const not = yield* Retry.toRawPredicate(Retry.Predicate.always().not())
      expect(Retry.evaluatePredicate(t, {})).toBe(true)
      expect(Retry.evaluatePredicate(f, {})).toBe(false)
      expect(Retry.evaluatePredicate(not, {})).toBe(false)
    }).pipe(Effect.runSync))

  it("supports glob matching via prop-matches", () =>
    Effect.gen(function* () {
      const pred = yield* Retry.toRawPredicate(
        Retry.Predicate.matchesGlob(Retry.Props.uriPath, "/api/*/users"),
      )
      expect(Retry.evaluatePredicate(pred, { "uri-path": "/api/v1/users" })).toBe(true)
      expect(Retry.evaluatePredicate(pred, { "uri-path": "/api/v2/users" })).toBe(true)
      expect(Retry.evaluatePredicate(pred, { "uri-path": "/api/v1/posts" })).toBe(false)
    }).pipe(Effect.runSync))
})
