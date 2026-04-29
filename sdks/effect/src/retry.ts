import { Duration, Effect, Scope } from "effect"
import type * as RetryHost from "golem:api/retry@1.5.0"
import { RetryClient } from "./host/RetryClient.js"

/**
 * Effect-idiomatic façade over `golem:api/retry@1.5.0`.
 *
 * Mirrors the official `golem-ts-sdk` retry DSL — `Policy`, `Predicate`,
 * `NamedPolicy`, `Props` — but exposes every host interaction as an
 * Effect with typed failures and offers a scoped helper that integrates
 * with `Scope` so retry policies can be activated within a fiber's
 * lifetime.
 *
 * Authoring model:
 *
 * ```ts
 * import { Retry } from "effect-golem"
 * import { Duration, Effect } from "effect"
 *
 * const policy = Retry.NamedPolicy.named(
 *   "http-transient",
 *   Retry.Policy.exponential(Duration.seconds(1), 2)
 *     .maxRetries(5)
 *     .withJitter(0.1)
 *     .onlyWhen(
 *       Retry.Predicate.gte(Retry.Props.statusCode, 500).and(
 *         Retry.Predicate.neq(Retry.Props.statusCode, 501),
 *       ),
 *     ),
 * ).priority(10)
 *
 * // Imperative, persisted to oplog:
 * yield* Retry.setPolicy(policy)
 *
 * // Scoped: only active for the wrapped Effect's lifetime,
 * // then the previous policy (or absence thereof) is restored:
 * yield* Retry.withPolicy(policy, doWork)
 * ```
 *
 * The DSL is bit-for-bit compatible with the official `golem-ts-sdk`
 * because the flatten algorithm and validation rules match — see
 * `toRaw*` for the wire conversion.
 */

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when a policy or predicate cannot be flattened to its WIT shape
 * because some user-supplied value is out of range, non-finite, or
 * otherwise malformed (e.g. `Policy.exponential` with a non-positive
 * factor, `Policy.clamp` with `minDelay > maxDelay`, a duration that
 * doesn't fit a `u64`, a numeric predicate value that doesn't fit an
 * `i64`).
 */
export class RetryPolicyValidationError {
  readonly _tag = "RetryPolicyValidationError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `RetryPolicyValidationError: ${reason}`
  }
}

/**
 * Raised when a `golem:api/retry@1.5.0` host call throws unexpectedly.
 * The host calls are oplog-persisted on the Golem side; this only fires
 * on hard runtime failures (e.g. the binding is missing).
 */
export class RetryHostError {
  readonly _tag = "RetryHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `RetryHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Internal: predicate / policy AST
// ---------------------------------------------------------------------------

/** Allowed leaf values in property-comparison predicates. */
export type PredicateValueInput = string | boolean | bigint | number

const INT64_MIN = -(1n << 63n)
const INT64_MAX = (1n << 63n) - 1n
const UINT32_MAX = 0xffff_ffff
const UINT64_MAX = (1n << 64n) - 1n

type PredicateNodeDef =
  | { readonly tag: "pred-true" }
  | { readonly tag: "pred-false" }
  | {
      readonly tag: "prop-eq" | "prop-neq" | "prop-gt" | "prop-gte" | "prop-lt" | "prop-lte"
      readonly property: string
      readonly value: PredicateValueInput
    }
  | { readonly tag: "prop-exists"; readonly property: string }
  | {
      readonly tag: "prop-in"
      readonly property: string
      readonly values: ReadonlyArray<PredicateValueInput>
    }
  | {
      readonly tag: "prop-matches" | "prop-starts-with" | "prop-contains"
      readonly property: string
      readonly pattern: string
    }
  | {
      readonly tag: "pred-and" | "pred-or"
      readonly left: PredicateNodeDef
      readonly right: PredicateNodeDef
    }
  | { readonly tag: "pred-not"; readonly inner: PredicateNodeDef }

type PolicyNodeDef =
  | { readonly tag: "immediate" }
  | { readonly tag: "never" }
  | { readonly tag: "periodic"; readonly delay: Duration.Input }
  | {
      readonly tag: "exponential"
      readonly baseDelay: Duration.Input
      readonly factor: number
    }
  | {
      readonly tag: "fibonacci"
      readonly first: Duration.Input
      readonly second: Duration.Input
    }
  | {
      readonly tag: "count-box"
      readonly maxRetries: number
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "time-box"
      readonly limit: Duration.Input
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "clamp-delay"
      readonly minDelay: Duration.Input
      readonly maxDelay: Duration.Input
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "add-delay"
      readonly delay: Duration.Input
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "jitter"
      readonly factor: number
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "filtered-on"
      readonly predicate: PredicateNodeDef
      readonly inner: PolicyNodeDef
    }
  | {
      readonly tag: "and-then" | "policy-union" | "policy-intersect"
      readonly left: PolicyNodeDef
      readonly right: PolicyNodeDef
    }

// ---------------------------------------------------------------------------
// Predicate AST
// ---------------------------------------------------------------------------

/**
 * Immutable predicate AST. Compiles down to the flattened
 * `golem:api/retry@1.5.0` `retry-predicate` shape via {@link toRawPredicate}.
 *
 * Predicates select *when* a policy applies. The host evaluates them
 * against per-operation context properties (see {@link Props}).
 */
export class Predicate {
  /** @internal */
  private constructor(private readonly node: PredicateNodeDef) {}

  /** Always-true predicate (`pred-true`). */
  static always(): Predicate {
    return new Predicate({ tag: "pred-true" })
  }

  /** Always-false predicate (`pred-false`). */
  static never(): Predicate {
    return new Predicate({ tag: "pred-false" })
  }

  /** `property == value`. */
  static eq(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-eq", property, value })
  }

  /** `property != value`. */
  static neq(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-neq", property, value })
  }

  /** `property > value`. */
  static gt(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-gt", property, value })
  }

  /** `property >= value`. */
  static gte(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-gte", property, value })
  }

  /** `property < value`. */
  static lt(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-lt", property, value })
  }

  /** `property <= value`. */
  static lte(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: "prop-lte", property, value })
  }

  /** Property is present in the context. */
  static exists(property: string): Predicate {
    return new Predicate({ tag: "prop-exists", property })
  }

  /** Property value equals any one of `values` (`prop-in`). */
  static oneOf(property: string, values: ReadonlyArray<PredicateValueInput>): Predicate {
    return new Predicate({ tag: "prop-in", property, values })
  }

  /** Glob-pattern match against a string property (`prop-matches`). */
  static matchesGlob(property: string, pattern: string): Predicate {
    return new Predicate({ tag: "prop-matches", property, pattern })
  }

  /** String property starts with `prefix` (`prop-starts-with`). */
  static startsWith(property: string, prefix: string): Predicate {
    return new Predicate({ tag: "prop-starts-with", property, pattern: prefix })
  }

  /** String property contains `substring` (`prop-contains`). */
  static contains(property: string, substring: string): Predicate {
    return new Predicate({ tag: "prop-contains", property, pattern: substring })
  }

  /** Boolean conjunction. */
  and(other: Predicate): Predicate {
    return new Predicate({ tag: "pred-and", left: this.node, right: other.node })
  }

  /** Boolean disjunction. */
  or(other: Predicate): Predicate {
    return new Predicate({ tag: "pred-or", left: this.node, right: other.node })
  }

  /** Boolean negation. */
  not(): Predicate {
    return new Predicate({ tag: "pred-not", inner: this.node })
  }

  /** @internal — read the AST root for embedding in a {@link Policy}. */
  toNode(): PredicateNodeDef {
    return this.node
  }
}

// ---------------------------------------------------------------------------
// Policy AST
// ---------------------------------------------------------------------------

/**
 * Immutable retry-policy AST. Compiles down to the flattened
 * `golem:api/retry@1.5.0` `retry-policy` shape via {@link toRawPolicy}.
 *
 * Policies describe *how* to retry: leaf strategies (`immediate`,
 * `periodic`, `exponential`, `fibonacci`, `never`) wrapped by
 * combinators (`maxRetries`, `within`, `clamp`, `addDelay`,
 * `withJitter`, `onlyWhen`) and joined via `andThen` / `union` /
 * `intersect`.
 */
export class Policy {
  /** @internal */
  private constructor(private readonly node: PolicyNodeDef) {}

  /** Retry immediately, no delay. */
  static immediate(): Policy {
    return new Policy({ tag: "immediate" })
  }

  /** Never retry — terminates as soon as it's selected. */
  static never(): Policy {
    return new Policy({ tag: "never" })
  }

  /** Retry on a fixed `delay` interval. */
  static periodic(delay: Duration.Input): Policy {
    return new Policy({ tag: "periodic", delay })
  }

  /**
   * Exponential backoff: `delay = baseDelay * factor^attempt`. `factor`
   * must be a finite number greater than 0.
   */
  static exponential(baseDelay: Duration.Input, factor: number): Policy {
    return new Policy({ tag: "exponential", baseDelay, factor })
  }

  /** Fibonacci backoff seeded with `first` and `second`. */
  static fibonacci(first: Duration.Input, second: Duration.Input): Policy {
    return new Policy({ tag: "fibonacci", first, second })
  }

  /** Wrap the current policy with a max-retry count (u32). */
  maxRetries(maxRetries: number): Policy {
    return new Policy({ tag: "count-box", maxRetries, inner: this.node })
  }

  /** Wrap the current policy with a total-time bound. */
  within(limit: Duration.Input): Policy {
    return new Policy({ tag: "time-box", limit, inner: this.node })
  }

  /** Clamp generated delays into `[minDelay, maxDelay]`. */
  clamp(minDelay: Duration.Input, maxDelay: Duration.Input): Policy {
    return new Policy({ tag: "clamp-delay", minDelay, maxDelay, inner: this.node })
  }

  /** Add a fixed `delay` on top of the current policy's output. */
  addDelay(delay: Duration.Input): Policy {
    return new Policy({ tag: "add-delay", delay, inner: this.node })
  }

  /** Multiplicative jitter (`factor` ≥ 0, finite). */
  withJitter(factor: number): Policy {
    return new Policy({ tag: "jitter", factor, inner: this.node })
  }

  /** Only apply this policy when `predicate` matches the operation context. */
  onlyWhen(predicate: Predicate): Policy {
    return new Policy({ tag: "filtered-on", predicate: predicate.toNode(), inner: this.node })
  }

  /** Apply this policy until exhausted, then fall back to `other`. */
  andThen(other: Policy): Policy {
    return new Policy({ tag: "and-then", left: this.node, right: other.node })
  }

  /** Pick the more permissive of two policies (logical OR over retry budgets). */
  union(other: Policy): Policy {
    return new Policy({ tag: "policy-union", left: this.node, right: other.node })
  }

  /** Pick the more restrictive of two policies (logical AND over retry budgets). */
  intersect(other: Policy): Policy {
    return new Policy({ tag: "policy-intersect", left: this.node, right: other.node })
  }

  /** @internal — read the AST root. */
  toNode(): PolicyNodeDef {
    return this.node
  }
}

// ---------------------------------------------------------------------------
// NamedPolicy
// ---------------------------------------------------------------------------

const NAMED_POLICY_TYPE_ID = Symbol.for("effect-golem/Retry/NamedPolicy")

/**
 * A named, prioritised retry policy. The host stores the set of named
 * policies (persisted to the oplog) and selects one per operation by
 * evaluating each rule's predicate in descending priority order.
 *
 * Build with `NamedPolicy.named(name, policy)`. Defaults: priority `0`,
 * predicate `Predicate.always()`. Both can be overridden with the
 * (immutable) `priority(...)` and `appliesWhen(...)` chain methods.
 */
export class NamedPolicy {
  /** @internal */
  readonly [NAMED_POLICY_TYPE_ID] = true

  /** @internal */
  private constructor(
    readonly name: string,
    private readonly policyAst: Policy,
    private readonly predicateAst: Predicate,
    private readonly policyPriority: number,
  ) {}

  /** Construct a named policy with default priority `0` and `always()` predicate. */
  static named(name: string, policy: Policy): NamedPolicy {
    return new NamedPolicy(name, policy, Predicate.always(), 0)
  }

  /** Replace the priority (u32; higher = evaluated first). */
  priority(priority: number): NamedPolicy {
    return new NamedPolicy(this.name, this.policyAst, this.predicateAst, priority)
  }

  /** Replace the predicate that selects this rule. */
  appliesWhen(predicate: Predicate): NamedPolicy {
    return new NamedPolicy(this.name, this.policyAst, predicate, this.policyPriority)
  }

  /** @internal — used by {@link toRawNamedPolicy}. */
  parts(): {
    readonly name: string
    readonly policy: Policy
    readonly predicate: Predicate
    readonly priority: number
  } {
    return {
      name: this.name,
      policy: this.policyAst,
      predicate: this.predicateAst,
      priority: this.policyPriority,
    }
  }
}

const isNamedPolicy = (value: unknown): value is NamedPolicy =>
  typeof value === "object" &&
  value !== null &&
  (value as { readonly [NAMED_POLICY_TYPE_ID]?: unknown })[NAMED_POLICY_TYPE_ID] === true

/** Either flavour accepted by the high-level setters / scoping helpers. */
export type NamedPolicyInput = NamedPolicy | RetryHost.NamedRetryPolicy

// ---------------------------------------------------------------------------
// Well-known property keys
// ---------------------------------------------------------------------------

/**
 * Well-known property names exposed by the Golem host in retry contexts.
 * Mirrors the official `golem-ts-sdk` `Props` constants.
 */
export const Props = {
  verb: "verb",
  nounUri: "noun-uri",
  uriScheme: "uri-scheme",
  uriHost: "uri-host",
  uriPort: "uri-port",
  uriPath: "uri-path",
  statusCode: "status-code",
  errorType: "error-type",
  function: "function",
  targetComponentId: "target-component-id",
  targetAgentType: "target-agent-type",
  dbType: "db-type",
  trapType: "trap-type",
} as const

// ---------------------------------------------------------------------------
// Conversion to raw WIT shape (Effect-typed validation)
// ---------------------------------------------------------------------------

const fail = <A>(reason: string): Effect.Effect<A, RetryPolicyValidationError> =>
  Effect.fail(new RetryPolicyValidationError(reason))

const toBigIntInteger = (
  value: bigint | number,
  context: string,
): Effect.Effect<bigint, RetryPolicyValidationError> => {
  if (typeof value === "bigint") return Effect.succeed(value)
  if (!Number.isSafeInteger(value)) {
    return fail(`${context} must be a safe integer or bigint (got ${String(value)})`)
  }
  return Effect.succeed(BigInt(value))
}

const toInt64 = (
  value: bigint | number,
  context: string,
): Effect.Effect<bigint, RetryPolicyValidationError> =>
  toBigIntInteger(value, context).pipe(
    Effect.flatMap((n) =>
      n < INT64_MIN || n > INT64_MAX
        ? fail(`${context} must fit a signed 64-bit integer (got ${n.toString()})`)
        : Effect.succeed(n),
    ),
  )

const toRawDuration = (
  value: Duration.Input,
  context: string,
): Effect.Effect<bigint, RetryPolicyValidationError> => {
  let nanos: bigint
  try {
    const dur = Duration.fromInputUnsafe(value)
    nanos = Duration.toNanosUnsafe(dur)
  } catch (e) {
    return fail(`${context}: ${e instanceof Error ? e.message : String(e)}`)
  }
  if (nanos < 0n) return fail(`${context} must be non-negative (got ${nanos.toString()}ns)`)
  if (nanos > UINT64_MAX) return fail(`${context} must fit an unsigned 64-bit duration`)
  return Effect.succeed(nanos)
}

const ensureUint32 = (
  value: number,
  context: string,
): Effect.Effect<number, RetryPolicyValidationError> => {
  if (!Number.isSafeInteger(value) || value < 0 || value > UINT32_MAX) {
    return fail(`${context} must be a non-negative 32-bit integer (got ${String(value)})`)
  }
  return Effect.succeed(value)
}

const ensureFinitePositiveNumber = (
  value: number,
  context: string,
): Effect.Effect<number, RetryPolicyValidationError> =>
  Number.isFinite(value) && value > 0
    ? Effect.succeed(value)
    : fail(`${context} must be a finite number greater than 0 (got ${String(value)})`)

const ensureFiniteNonNegativeNumber = (
  value: number,
  context: string,
): Effect.Effect<number, RetryPolicyValidationError> =>
  Number.isFinite(value) && value >= 0
    ? Effect.succeed(value)
    : fail(`${context} must be a finite number greater than or equal to 0 (got ${String(value)})`)

const toRawPredicateValue = (
  value: PredicateValueInput,
  context: string,
): Effect.Effect<RetryHost.PredicateValue, RetryPolicyValidationError> => {
  if (typeof value === "string") return Effect.succeed({ tag: "text", val: value })
  if (typeof value === "boolean") return Effect.succeed({ tag: "boolean", val: value })
  return toInt64(value, context).pipe(Effect.map((val) => ({ tag: "integer", val }) as const))
}

/** Encode a {@link PredicateValueInput} to its WIT `predicate-value` shape. */
export const encodePredicateValue = (
  value: PredicateValueInput,
): Effect.Effect<RetryHost.PredicateValue, RetryPolicyValidationError> =>
  toRawPredicateValue(value, "predicate value")

const appendPredicateNode = (
  node: PredicateNodeDef,
  nodes: Array<RetryHost.PredicateNode>,
): Effect.Effect<number, RetryPolicyValidationError> =>
  Effect.gen(function* () {
    const index = nodes.length
    // Reserve a slot so child indices are stable.
    nodes.push({ tag: "pred-false" })

    switch (node.tag) {
      case "pred-true":
      case "pred-false":
        nodes[index] = { tag: node.tag }
        return index
      case "prop-eq":
      case "prop-neq":
      case "prop-gt":
      case "prop-gte":
      case "prop-lt":
      case "prop-lte": {
        const val = yield* toRawPredicateValue(
          node.value,
          `Predicate.${node.tag}(${node.property})`,
        )
        nodes[index] = {
          tag: node.tag,
          val: { propertyName: node.property, value: val },
        }
        return index
      }
      case "prop-exists":
        nodes[index] = { tag: "prop-exists", val: node.property }
        return index
      case "prop-in": {
        const values: Array<RetryHost.PredicateValue> = []
        for (let i = 0; i < node.values.length; i++) {
          values.push(
            yield* toRawPredicateValue(node.values[i], `Predicate.oneOf(${node.property})[${i}]`),
          )
        }
        nodes[index] = {
          tag: "prop-in",
          val: { propertyName: node.property, values },
        }
        return index
      }
      case "prop-matches":
      case "prop-starts-with":
      case "prop-contains":
        nodes[index] = {
          tag: node.tag,
          val: { propertyName: node.property, pattern: node.pattern },
        }
        return index
      case "pred-and":
      case "pred-or": {
        const left = yield* appendPredicateNode(node.left, nodes)
        const right = yield* appendPredicateNode(node.right, nodes)
        nodes[index] = { tag: node.tag, val: [left, right] }
        return index
      }
      case "pred-not": {
        const inner = yield* appendPredicateNode(node.inner, nodes)
        nodes[index] = { tag: "pred-not", val: inner }
        return index
      }
    }
  })

const appendPolicyNode = (
  node: PolicyNodeDef,
  nodes: Array<RetryHost.PolicyNode>,
): Effect.Effect<number, RetryPolicyValidationError> =>
  Effect.gen(function* () {
    const index = nodes.length
    nodes.push({ tag: "never" })

    switch (node.tag) {
      case "immediate":
      case "never":
        nodes[index] = { tag: node.tag }
        return index
      case "periodic": {
        const val = yield* toRawDuration(node.delay, "Policy.periodic")
        nodes[index] = { tag: "periodic", val }
        return index
      }
      case "exponential": {
        const baseDelay = yield* toRawDuration(node.baseDelay, "Policy.exponential.baseDelay")
        const factor = yield* ensureFinitePositiveNumber(node.factor, "Policy.exponential.factor")
        nodes[index] = { tag: "exponential", val: { baseDelay, factor } }
        return index
      }
      case "fibonacci": {
        const first = yield* toRawDuration(node.first, "Policy.fibonacci.first")
        const second = yield* toRawDuration(node.second, "Policy.fibonacci.second")
        nodes[index] = { tag: "fibonacci", val: { first, second } }
        return index
      }
      case "count-box": {
        const inner = yield* appendPolicyNode(node.inner, nodes)
        const maxRetries = yield* ensureUint32(node.maxRetries, "Policy.maxRetries")
        nodes[index] = { tag: "count-box", val: { maxRetries, inner } }
        return index
      }
      case "time-box": {
        const inner = yield* appendPolicyNode(node.inner, nodes)
        const limit = yield* toRawDuration(node.limit, "Policy.within.limit")
        nodes[index] = { tag: "time-box", val: { limit, inner } }
        return index
      }
      case "clamp-delay": {
        const minDelay = yield* toRawDuration(node.minDelay, "Policy.clamp.minDelay")
        const maxDelay = yield* toRawDuration(node.maxDelay, "Policy.clamp.maxDelay")
        if (minDelay > maxDelay) {
          return yield* Effect.fail(
            new RetryPolicyValidationError(
              "Policy.clamp requires minDelay to be less than or equal to maxDelay",
            ),
          )
        }
        const inner = yield* appendPolicyNode(node.inner, nodes)
        nodes[index] = { tag: "clamp-delay", val: { minDelay, maxDelay, inner } }
        return index
      }
      case "add-delay": {
        const inner = yield* appendPolicyNode(node.inner, nodes)
        const delay = yield* toRawDuration(node.delay, "Policy.addDelay.delay")
        nodes[index] = { tag: "add-delay", val: { delay, inner } }
        return index
      }
      case "jitter": {
        const inner = yield* appendPolicyNode(node.inner, nodes)
        const factor = yield* ensureFiniteNonNegativeNumber(node.factor, "Policy.withJitter.factor")
        nodes[index] = { tag: "jitter", val: { factor, inner } }
        return index
      }
      case "filtered-on": {
        const inner = yield* appendPolicyNode(node.inner, nodes)
        const predicate = yield* buildRawPredicate(node.predicate)
        nodes[index] = { tag: "filtered-on", val: { predicate, inner } }
        return index
      }
      case "and-then":
      case "policy-union":
      case "policy-intersect": {
        const left = yield* appendPolicyNode(node.left, nodes)
        const right = yield* appendPolicyNode(node.right, nodes)
        nodes[index] = { tag: node.tag, val: [left, right] }
        return index
      }
    }
  })

const buildRawPredicate = (
  root: PredicateNodeDef,
): Effect.Effect<RetryHost.RetryPredicate, RetryPolicyValidationError> =>
  Effect.gen(function* () {
    const nodes: Array<RetryHost.PredicateNode> = []
    yield* appendPredicateNode(root, nodes)
    return { nodes }
  })

const buildRawPolicy = (
  root: PolicyNodeDef,
): Effect.Effect<RetryHost.RetryPolicy, RetryPolicyValidationError> =>
  Effect.gen(function* () {
    const nodes: Array<RetryHost.PolicyNode> = []
    yield* appendPolicyNode(root, nodes)
    return { nodes }
  })

/** Compile a {@link Predicate} to its flattened WIT shape. */
export const toRawPredicate = (
  predicate: Predicate,
): Effect.Effect<RetryHost.RetryPredicate, RetryPolicyValidationError> =>
  buildRawPredicate(predicate.toNode())

/** Compile a {@link Policy} to its flattened WIT shape. */
export const toRawPolicy = (
  policy: Policy,
): Effect.Effect<RetryHost.RetryPolicy, RetryPolicyValidationError> =>
  buildRawPolicy(policy.toNode())

/**
 * Compile a {@link NamedPolicy} (or pass through a raw
 * `NamedRetryPolicy`) to its WIT shape. Validates the priority is a
 * `u32` and the predicate / policy trees are well-formed.
 */
export const toRawNamedPolicy = (
  policy: NamedPolicyInput,
): Effect.Effect<RetryHost.NamedRetryPolicy, RetryPolicyValidationError> => {
  if (!isNamedPolicy(policy)) return Effect.succeed(policy)
  const { name, policy: policyAst, predicate, priority } = policy.parts()
  return Effect.gen(function* () {
    const priorityU32 = yield* ensureUint32(priority, "NamedPolicy.priority")
    const compiledPredicate = yield* buildRawPredicate(predicate.toNode())
    const compiledPolicy = yield* buildRawPolicy(policyAst.toNode())
    return {
      name,
      priority: priorityU32,
      predicate: compiledPredicate,
      policy: compiledPolicy,
    }
  })
}

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/** Get all named retry policies currently active on this agent. */
export const getPolicies = (): Effect.Effect<
  ReadonlyArray<RetryHost.NamedRetryPolicy>,
  RetryHostError,
  RetryClient
> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    return yield* Effect.try({
      try: () => client.getRetryPolicies(),
      catch: (e) => new RetryHostError(e),
    })
  })

/** Look up a single named policy. Resolves to `undefined` when no rule with that name exists. */
export const getPolicyByName = (
  name: string,
): Effect.Effect<RetryHost.NamedRetryPolicy | undefined, RetryHostError, RetryClient> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    return yield* Effect.try({
      try: () => client.getRetryPolicyByName(name),
      catch: (e) => new RetryHostError(e),
    })
  })

/**
 * Resolve the matching retry policy for a given operation context. The
 * host evaluates the named policies in descending priority order and
 * returns the first match (or `undefined` when none match).
 *
 * `properties` accepts the high-level `PredicateValueInput` form
 * (`string | bigint | number | boolean`) — the SDK encodes it to the
 * WIT `predicate-value` variant for you.
 */
export const resolvePolicy = (
  verb: string,
  nounUri: string,
  properties: ReadonlyArray<readonly [string, PredicateValueInput]>,
): Effect.Effect<
  RetryHost.RetryPolicy | undefined,
  RetryHostError | RetryPolicyValidationError,
  RetryClient
> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    const encoded: Array<readonly [string, RetryHost.PredicateValue]> = []
    for (let i = 0; i < properties.length; i++) {
      const [k, v] = properties[i]
      const val = yield* toRawPredicateValue(v, `resolvePolicy.properties[${i}](${k})`)
      encoded.push([k, val])
    }
    return yield* Effect.try({
      try: () => client.resolveRetryPolicy(verb, nounUri, encoded),
      catch: (e) => new RetryHostError(e),
    })
  })

/**
 * Add or overwrite a named retry policy. Mirrors the host's
 * `set-retry-policy` (oplog-persisted on durable agents).
 */
export const setPolicy = (
  policy: NamedPolicyInput,
): Effect.Effect<void, RetryPolicyValidationError | RetryHostError, RetryClient> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    const raw = yield* toRawNamedPolicy(policy)
    yield* Effect.try({
      try: () => client.setRetryPolicy(raw),
      catch: (e) => new RetryHostError(e),
    })
  })

/** Remove a named retry policy. Mirrors the host's `remove-retry-policy`. */
export const removePolicy = (name: string): Effect.Effect<void, RetryHostError, RetryClient> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    return yield* Effect.try({
      try: () => client.removeRetryPolicy(name),
      catch: (e) => new RetryHostError(e),
    })
  })

// ---------------------------------------------------------------------------
// Scoped activation
// ---------------------------------------------------------------------------

/**
 * Set a named retry policy and arrange for the previous policy to be
 * restored when the surrounding {@link Scope} closes. If no policy with
 * the same name existed, the policy is removed on close.
 *
 * Use directly with `Effect.scoped`, or via {@link withPolicy} for the
 * common "run this Effect with a temporary policy" shape.
 */
export const useScoped = (
  policy: NamedPolicyInput,
): Effect.Effect<void, RetryPolicyValidationError | RetryHostError, Scope.Scope | RetryClient> =>
  Effect.gen(function* () {
    const client = yield* RetryClient
    const raw = yield* toRawNamedPolicy(policy)
    const previous = yield* Effect.try({
      try: () => client.getRetryPolicyByName(raw.name),
      catch: (e) => new RetryHostError(e),
    })
    yield* Effect.acquireRelease(
      Effect.try({ try: () => client.setRetryPolicy(raw), catch: (e) => new RetryHostError(e) }),
      () =>
        Effect.try({
          try: () =>
            previous !== undefined
              ? client.setRetryPolicy(previous)
              : client.removeRetryPolicy(raw.name),
          catch: () => undefined,
        }).pipe(Effect.ignore),
    )
  })

/**
 * Run `effect` with `policy` temporarily installed. Equivalent to
 * `Effect.scoped(useScoped(policy).pipe(Effect.zipRight(effect)))`. The
 * previous policy (or absence thereof) is restored on success, error,
 * and interruption alike.
 */
export const withPolicy = <A, E, R>(
  policy: NamedPolicyInput,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<
  A,
  E | RetryPolicyValidationError | RetryHostError,
  Exclude<R, Scope.Scope> | RetryClient
> => Effect.scoped(useScoped(policy).pipe(Effect.andThen(effect)))

// ---------------------------------------------------------------------------
// Re-exports of raw WIT types (no re-export of the host functions; use
// the Effect-typed wrappers above instead).
// ---------------------------------------------------------------------------

export type {
  NamedRetryPolicy,
  PolicyNode,
  PredicateNode,
  PredicateValue,
  RetryPolicy,
  RetryPredicate,
} from "golem:api/retry@1.5.0"
