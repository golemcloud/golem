/**
 * Runtime mock for `golem:api/retry@1.5.0`. Holds an in-memory map of
 * named policies; tests can reset / inspect it via the helpers below.
 *
 * `src/retry.ts` does not call these directly — it goes through
 * module-level `*Impl` indirections which the unit tests stub out
 * explicitly. The mock exists primarily so that the module imports
 * resolve at all under vitest.
 */

export type Duration = bigint

export type PredicateValue =
  | { tag: "text"; val: string }
  | { tag: "integer"; val: bigint }
  | { tag: "boolean"; val: boolean }

export type PropertyComparison = {
  propertyName: string
  value: PredicateValue
}

export type PropertySetCheck = {
  propertyName: string
  values: PredicateValue[]
}

export type PropertyPattern = {
  propertyName: string
  pattern: string
}

export type PredicateNodeIndex = number

export type PredicateNode =
  | {
      tag: "prop-eq" | "prop-neq" | "prop-gt" | "prop-gte" | "prop-lt" | "prop-lte"
      val: PropertyComparison
    }
  | { tag: "prop-exists"; val: string }
  | { tag: "prop-in"; val: PropertySetCheck }
  | { tag: "prop-matches" | "prop-starts-with" | "prop-contains"; val: PropertyPattern }
  | { tag: "pred-and" | "pred-or"; val: [PredicateNodeIndex, PredicateNodeIndex] }
  | { tag: "pred-not"; val: PredicateNodeIndex }
  | { tag: "pred-true" }
  | { tag: "pred-false" }

export type RetryPredicate = { nodes: PredicateNode[] }

export type ExponentialConfig = { baseDelay: Duration; factor: number }
export type FibonacciConfig = { first: Duration; second: Duration }
export type CountBoxConfig = { maxRetries: number; inner: PolicyNodeIndex }
export type TimeBoxConfig = { limit: Duration; inner: PolicyNodeIndex }
export type ClampConfig = { minDelay: Duration; maxDelay: Duration; inner: PolicyNodeIndex }
export type AddDelayConfig = { delay: Duration; inner: PolicyNodeIndex }
export type JitterConfig = { factor: number; inner: PolicyNodeIndex }
export type FilteredConfig = { predicate: RetryPredicate; inner: PolicyNodeIndex }

export type PolicyNodeIndex = number

export type PolicyNode =
  | { tag: "periodic"; val: Duration }
  | { tag: "exponential"; val: ExponentialConfig }
  | { tag: "fibonacci"; val: FibonacciConfig }
  | { tag: "immediate" }
  | { tag: "never" }
  | { tag: "count-box"; val: CountBoxConfig }
  | { tag: "time-box"; val: TimeBoxConfig }
  | { tag: "clamp-delay"; val: ClampConfig }
  | { tag: "add-delay"; val: AddDelayConfig }
  | { tag: "jitter"; val: JitterConfig }
  | { tag: "filtered-on"; val: FilteredConfig }
  | {
      tag: "and-then" | "policy-union" | "policy-intersect"
      val: [PolicyNodeIndex, PolicyNodeIndex]
    }

export type RetryPolicy = { nodes: PolicyNode[] }

export type NamedRetryPolicy = {
  name: string
  priority: number
  predicate: RetryPredicate
  policy: RetryPolicy
}

const policies = new Map<string, NamedRetryPolicy>()

export const __reset = (): void => {
  policies.clear()
}

export const __snapshot = (): ReadonlyMap<string, NamedRetryPolicy> => new Map(policies)

export const getRetryPolicies = (): NamedRetryPolicy[] => [...policies.values()]

export const getRetryPolicyByName = (name: string): NamedRetryPolicy | undefined =>
  policies.get(name)

export const resolveRetryPolicy = (
  _verb: string,
  _nounUri: string,
  _properties: [string, PredicateValue][],
): RetryPolicy | undefined => undefined

export const setRetryPolicy = (policy: NamedRetryPolicy): void => {
  policies.set(policy.name, policy)
}

export const removeRetryPolicy = (name: string): void => {
  policies.delete(name)
}
