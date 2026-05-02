/**
 * Internal: convert a Golem retry policy AST into an Effect `Schedule`.
 *
 * Public surface lives on `src/Retry.ts`.
 *
 * @since 1.5.0
 */

import { Duration, Effect, Random, Schedule } from "effect"
import type * as RetryHost from "golem:api/retry@1.5.0"
import {
  Policy,
  type PredicateValueInput,
  RetryPolicyValidationError,
  toRawPolicy,
} from "../Retry.js"

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Mapping from property name to the value seen by the predicate
 * evaluator. Property values follow the same coercion rules as
 * {@link PredicateValueInput} on the construction side.
 *
 * @since 1.5.0
 * @category models
 */
export interface PredicateContext {
  readonly [property: string]: PredicateValueInput
}

/**
 * Optional inputs to {@link toSchedule}.
 *
 * `properties` is consulted when the policy contains a `filtered-on`
 * (a.k.a. `Policy.onlyWhen(...)`) sub-tree: the function projects the
 * Effect's failure value (the `In` input of `Effect.retry(eff, sched)`)
 * to the property bag the predicate evaluates against. Omit it when the
 * policy has no predicate gates — the default returns an empty bag, which
 * causes `prop-exists`/`prop-eq`/etc. to report `false`.
 *
 * @since 1.5.0
 * @category models
 */
export interface ToScheduleOptions<In> {
  readonly properties?: (input: In) => PredicateContext
}

// ---------------------------------------------------------------------------
// Predicate evaluator (raw form)
// ---------------------------------------------------------------------------

/**
 * Evaluate a flattened {@link RetryHost.RetryPredicate} against an
 * in-memory property bag.
 *
 * This is the local mirror of the host's predicate evaluator and is
 * used by {@link toSchedule} to decide whether `filtered-on` sub-trees
 * fire. Numeric comparisons are bigint-safe; `prop-matches` uses a
 * minimal glob (`*` = any sequence, `?` = any single char) — for
 * authoritative evaluation always rely on the host (e.g. via
 * `Retry.useScoped` / `Retry.withPolicy`).
 *
 * @since 1.5.0
 * @category operations
 */
export const evaluatePredicate = (
  predicate: RetryHost.RetryPredicate,
  props: PredicateContext,
): boolean => {
  if (predicate.nodes.length === 0) return false
  const evalNode = (idx: number): boolean => {
    const node = predicate.nodes[idx]
    if (!node) return false
    switch (node.tag) {
      case "pred-true":
        return true
      case "pred-false":
        return false
      case "pred-and":
        return evalNode(node.val[0]) && evalNode(node.val[1])
      case "pred-or":
        return evalNode(node.val[0]) || evalNode(node.val[1])
      case "pred-not":
        return !evalNode(node.val)
      case "prop-exists":
        return Object.prototype.hasOwnProperty.call(props, node.val)
      case "prop-eq":
      case "prop-neq":
      case "prop-gt":
      case "prop-gte":
      case "prop-lt":
      case "prop-lte": {
        const propVal = props[node.val.propertyName]
        if (propVal === undefined) return false
        return compareValues(node.tag, propVal, node.val.value)
      }
      case "prop-in": {
        const propVal = props[node.val.propertyName]
        if (propVal === undefined) return false
        return node.val.values.some((wv) => compareValues("prop-eq", propVal, wv))
      }
      case "prop-matches":
      case "prop-starts-with":
      case "prop-contains": {
        const propVal = props[node.val.propertyName]
        if (typeof propVal !== "string") return false
        return matchPattern(node.tag, propVal, node.val.pattern)
      }
    }
  }
  return evalNode(0)
}

const valueToJs = (v: RetryHost.PredicateValue): string | bigint | boolean => {
  switch (v.tag) {
    case "text":
      return v.val
    case "integer":
      return v.val
    case "boolean":
      return v.val
  }
}

const toBigIntSafe = (v: PredicateValueInput | bigint): bigint | undefined => {
  if (typeof v === "bigint") return v
  if (typeof v === "number" && Number.isSafeInteger(v)) return BigInt(v)
  return undefined
}

const compareValues = (
  op: "prop-eq" | "prop-neq" | "prop-gt" | "prop-gte" | "prop-lt" | "prop-lte",
  prop: PredicateValueInput,
  raw: RetryHost.PredicateValue,
): boolean => {
  const wit = valueToJs(raw)
  if (op === "prop-eq" || op === "prop-neq") {
    const eq = compareEq(prop, wit)
    return op === "prop-eq" ? eq : !eq
  }
  // Numeric ordering: only meaningful for integers.
  const a = toBigIntSafe(prop)
  const b = typeof wit === "bigint" ? wit : undefined
  if (a === undefined || b === undefined) return false
  switch (op) {
    case "prop-gt":
      return a > b
    case "prop-gte":
      return a >= b
    case "prop-lt":
      return a < b
    case "prop-lte":
      return a <= b
  }
}

const compareEq = (a: PredicateValueInput, b: string | bigint | boolean): boolean => {
  if (typeof a === "string" && typeof b === "string") return a === b
  if (typeof a === "boolean" && typeof b === "boolean") return a === b
  const an = toBigIntSafe(a)
  const bn = typeof b === "bigint" ? b : undefined
  if (an !== undefined && bn !== undefined) return an === bn
  return false
}

/**
 * Translate a glob (`*` = any sequence, `?` = exactly one) into a regex
 * source. All other regex metacharacters are escaped.
 */
const globToRegexSource = (pattern: string): string => {
  let src = "^"
  for (const ch of pattern) {
    if (ch === "*") src += ".*"
    else if (ch === "?") src += "."
    else src += ch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
  }
  return src + "$"
}

const matchPattern = (
  op: "prop-matches" | "prop-starts-with" | "prop-contains",
  prop: string,
  pattern: string,
): boolean => {
  switch (op) {
    case "prop-starts-with":
      return prop.startsWith(pattern)
    case "prop-contains":
      return prop.includes(pattern)
    case "prop-matches": {
      try {
        return new RegExp(globToRegexSource(pattern)).test(prop)
      } catch {
        return false
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Schedule builder (raw form)
// ---------------------------------------------------------------------------

const isPolicy = (value: Policy | RetryHost.RetryPolicy): value is Policy => value instanceof Policy

const nanosToDuration = (n: bigint): Duration.Duration => Duration.nanos(n)

/**
 * Produce an Effect `Schedule` from a Golem retry policy.
 *
 * Accepts either an SDK-side {@link Policy} (validated on the way in) or
 * a raw `RetryPolicy` returned by the host (e.g. via `getPolicyByName`,
 * `resolvePolicy`).
 *
 * The schedule produced runs entirely in JS — the host does NOT see
 * retries as `RetryAttempt` oplog entries. Use this when you want to
 * compose with `Schedule.tapInput` / `Schedule.zipRight` /
 * `Effect.retry`-with-typed-errors. Use {@link useScoped} /
 * {@link withPolicy} when you want host-driven retries that participate
 * in the durability oplog (e.g. cross-`SUSPEND` survival).
 *
 * **Node mapping:**
 *
 * - `immediate` → `Schedule.spaced(0)`
 * - `never` → `Schedule.recurs(0)` (returns Done on the first step)
 * - `periodic(d)` → `Schedule.spaced(d)`
 * - `exponential(b, k)` → `Schedule.exponential(b, k)`
 * - `fibonacci(a, b)` → custom step seeded with both values
 * - `count-box(n, inner)` → `Schedule.both(inner, Schedule.recurs(n))`
 * - `time-box(d, inner)` → `Schedule.both(inner, Schedule.during(d))`
 * - `clamp-delay(lo, hi, inner)` → `Schedule.modifyDelay(inner, clamp(lo, hi))`
 * - `add-delay(d, inner)` → `Schedule.addDelay(inner, () => d)`
 * - `jitter(f, inner)` → `Schedule.modifyDelay(inner, scale ∈ [1-f, 1+f))`
 * - `filtered-on(pred, inner)` → `Schedule.while(inner, props(input) ⊢ pred)`
 * - `and-then(a, b)` → `Schedule.andThen(a, b)`
 * - `policy-union(a, b)` → `Schedule.either(a, b)` (continues if either, MIN delay)
 * - `policy-intersect(a, b)` → `Schedule.both(a, b)` (continues if both, MAX delay)
 *
 * **Approximations vs. host execution** (the host is the canonical
 * truth; this is a JS-side transliteration):
 *
 * - `time-box(limit, inner)` is bounded by *elapsed* time (`elapsed <= limit`).
 *   The next attempt may therefore start AFTER the box expires when
 *   `inner`'s next delay overshoots the remaining budget. The host may
 *   choose to skip such attempts; this implementation does not.
 * - `filtered-on(pred, inner)` is `Schedule.while`: the predicate is
 *   re-evaluated on every step against the input projection. If it
 *   returns `false`, the inner subtree yields `Cause.done` for *that
 *   step only* — composed via `policy-union` / `policy-intersect` the
 *   neighbour's decision still wins, and on the next input whose
 *   predicate is `true` the subtree participates again. At the top
 *   level (no neighbour) "predicate false" terminates the schedule, as
 *   expected.
 * - `jitter(factor, ...)` samples uniformly on `[1-factor, 1+factor)`.
 *   For `factor > 1`, negative scales are clamped to `0`, which biases
 *   the distribution toward zero (a spike at zero rather than uniform).
 *   `factor = 0` is a no-op (delay unchanged).
 * - The local predicate evaluator does same-tag comparisons only
 *   (`text` ↔ `text`, `integer` ↔ `integer`, `boolean` ↔ `boolean`);
 *   pass `bigint` (not `number`) for integers outside JS's
 *   safe-integer range. Mixed-type comparisons return `false`.
 *
 * Use {@link useScoped} / {@link withPolicy} (host-driven retries) when
 * you need exact host parity.
 *
 * @since 1.5.0
 * @category combinators
 */
export const toSchedule = <In = unknown>(
  policy: Policy | RetryHost.RetryPolicy,
  opts?: ToScheduleOptions<In>,
): Effect.Effect<Schedule.Schedule<unknown, In>, RetryPolicyValidationError> =>
  Effect.gen(function* () {
    const raw = isPolicy(policy) ? yield* toRawPolicy(policy) : policy
    const props = opts?.properties ?? (() => ({}) as PredicateContext)
    return buildSchedule<In>(raw, 0, props)
  })

/**
 * Erase the schedule's `Output` type to `unknown` while preserving its
 * `Input`, `Error` and `Env` parameters. Used at every internal
 * recursion site so combinator outputs (`Duration.Duration`, `[A, B]`,
 * `number`, …) don't leak into the public `Schedule<unknown, In>`
 * return type.
 */
const erase = <Out, In, E, R>(
  s: Schedule.Schedule<Out, In, E, R>,
): Schedule.Schedule<unknown, In, E, R> => Schedule.map(s, (_) => undefined as unknown)

const buildSchedule = <In>(
  policy: RetryHost.RetryPolicy,
  rootIdx: number,
  props: (input: In) => PredicateContext,
): Schedule.Schedule<unknown, In> => {
  const node = policy.nodes[rootIdx]
  if (!node) {
    // Defensive: malformed policy — terminate immediately.
    return erase(Schedule.recurs(0))
  }
  switch (node.tag) {
    case "immediate":
      return erase(Schedule.spaced(Duration.zero))
    case "never":
      return erase(Schedule.recurs(0))
    case "periodic":
      return erase(Schedule.spaced(nanosToDuration(node.val)))
    case "exponential":
      return erase(Schedule.exponential(nanosToDuration(node.val.baseDelay), node.val.factor))
    case "fibonacci":
      return erase(makeFibonacciSchedule(node.val.first, node.val.second))
    case "count-box": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      return erase(Schedule.both(inner, Schedule.recurs(node.val.maxRetries)))
    }
    case "time-box": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      return erase(Schedule.both(inner, Schedule.during(nanosToDuration(node.val.limit))))
    }
    case "clamp-delay": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      const minimum = nanosToDuration(node.val.minDelay)
      const maximum = nanosToDuration(node.val.maxDelay)
      return erase(
        Schedule.modifyDelay(inner, (_o, delay) =>
          Effect.succeed(Duration.clamp(Duration.fromInputUnsafe(delay), { minimum, maximum })),
        ),
      )
    }
    case "add-delay": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      const extra = nanosToDuration(node.val.delay)
      return erase(Schedule.addDelay(inner, () => Effect.succeed(extra)))
    }
    case "jitter": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      const factor = node.val.factor
      return erase(
        Schedule.modifyDelay(inner, (_o, delay) =>
          Effect.map(Random.next, (r) => {
            // r ∈ [0, 1) → scale ∈ [1-f, 1+f)
            const scale = Math.max(0, 1 - factor + r * 2 * factor)
            // Multiply via millis to avoid `Duration.times`'s integer-bigint
            // coercion on nanos-backed Durations (rounds to whole millis).
            const ms = Duration.toMillis(Duration.fromInputUnsafe(delay))
            return Duration.millis(ms * scale)
          }),
        ),
      )
    }
    case "filtered-on": {
      const inner = buildSchedule<In>(policy, node.val.inner, props)
      const subPredicate = node.val.predicate
      return erase(
        Schedule.while(inner, ({ input }) => evaluatePredicate(subPredicate, props(input))),
      )
    }
    case "and-then": {
      const left = buildSchedule<In>(policy, node.val[0], props)
      const right = buildSchedule<In>(policy, node.val[1], props)
      return erase(Schedule.andThen(left, right))
    }
    case "policy-union": {
      const left = buildSchedule<In>(policy, node.val[0], props)
      const right = buildSchedule<In>(policy, node.val[1], props)
      return erase(Schedule.either(left, right))
    }
    case "policy-intersect": {
      const left = buildSchedule<In>(policy, node.val[0], props)
      const right = buildSchedule<In>(policy, node.val[1], props)
      return erase(Schedule.both(left, right))
    }
  }
}

/**
 * Two-seed Fibonacci: emits `first`, `second`, `first+second`,
 * `second + (first+second)`, … always sleeping for the just-emitted
 * delay. Sequence in nanoseconds (bigint) — millisecond conversion only
 * happens for the returned `Duration.Duration`.
 */
const makeFibonacciSchedule = (
  firstNanos: bigint,
  secondNanos: bigint,
): Schedule.Schedule<Duration.Duration> =>
  Schedule.fromStep(
    Effect.sync(() => {
      let a = firstNanos
      let b = secondNanos
      let n = 0
      return (_now: number, _input: unknown) =>
        Effect.sync(() => {
          let out: bigint
          if (n === 0) {
            out = a
            n = 1
          } else if (n === 1) {
            out = b
            n = 2
          } else {
            out = a + b
            a = b
            b = out
          }
          const dur = Duration.nanos(out)
          return [dur, dur] as const
        })
    }),
  )
