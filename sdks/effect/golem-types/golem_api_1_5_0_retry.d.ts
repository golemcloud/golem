declare module 'golem:api/retry@1.5.0' {
  import * as wasiClocks023MonotonicClock from 'wasi:clocks/monotonic-clock@0.2.3';
  /**
   * ── Host functions ───────────────────────────────────────────────
   * Get all retry policies active for this agent
   */
  export function getRetryPolicies(): NamedRetryPolicy[];
  /**
   * Get a specific retry policy by name
   */
  export function getRetryPolicyByName(name: string): NamedRetryPolicy | undefined;
  /**
   * Resolve the matching retry policy for a given operation context.
   * Evaluates named policies in descending priority order; returns the
   * policy from the first rule whose predicate matches, or none.
   */
  export function resolveRetryPolicy(verb: string, nounUri: string, properties: [string, PredicateValue][]): RetryPolicy | undefined;
  /**
   * Add or overwrite a named retry policy (persisted to oplog).
   * If a policy with the same name exists, it is replaced.
   */
  export function setRetryPolicy(policy: NamedRetryPolicy): void;
  /**
   * Remove a named retry policy by name (persisted to oplog).
   */
  export function removeRetryPolicy(name: string): void;
  export type Duration = wasiClocks023MonotonicClock.Duration;
  /**
   * ── Predicate value ──────────────────────────────────────────────
   * Dynamic value for property comparisons in retry predicates
   */
  export type PredicateValue = 
  {
    tag: 'text'
    val: string
  } |
  {
    tag: 'integer'
    val: bigint
  } |
  {
    tag: 'boolean'
    val: boolean
  };
  /**
   * ── Predicate tree (flattened) ───────────────────────────────────
   * Index into a retry-predicate's node list
   */
  export type PredicateNodeIndex = number;
  export type PropertyComparison = {
    propertyName: string;
    value: PredicateValue;
  };
  export type PropertySetCheck = {
    propertyName: string;
    values: PredicateValue[];
  };
  export type PropertyPattern = {
    propertyName: string;
    pattern: string;
  };
  export type PredicateNode = 
  {
    tag: 'prop-eq'
    val: PropertyComparison
  } |
  {
    tag: 'prop-neq'
    val: PropertyComparison
  } |
  {
    tag: 'prop-gt'
    val: PropertyComparison
  } |
  {
    tag: 'prop-gte'
    val: PropertyComparison
  } |
  {
    tag: 'prop-lt'
    val: PropertyComparison
  } |
  {
    tag: 'prop-lte'
    val: PropertyComparison
  } |
  {
    tag: 'prop-exists'
    val: string
  } |
  {
    tag: 'prop-in'
    val: PropertySetCheck
  } |
  {
    tag: 'prop-matches'
    val: PropertyPattern
  } |
  {
    tag: 'prop-starts-with'
    val: PropertyPattern
  } |
  {
    tag: 'prop-contains'
    val: PropertyPattern
  } |
  {
    tag: 'pred-and'
    val: [PredicateNodeIndex, PredicateNodeIndex]
  } |
  {
    tag: 'pred-or'
    val: [PredicateNodeIndex, PredicateNodeIndex]
  } |
  {
    tag: 'pred-not'
    val: PredicateNodeIndex
  } |
  {
    tag: 'pred-true'
  } |
  {
    tag: 'pred-false'
  };
  /**
   * A composable predicate tree for matching retry contexts.
   * Root is nodes[0]. Children referenced by predicate-node-index.
   */
  export type RetryPredicate = {
    nodes: PredicateNode[];
  };
  /**
   * ── Policy tree (flattened) ──────────────────────────────────────
   * Index into a retry-policy's node list
   */
  export type PolicyNodeIndex = number;
  export type ExponentialConfig = {
    baseDelay: Duration;
    factor: number;
  };
  export type FibonacciConfig = {
    first: Duration;
    second: Duration;
  };
  export type CountBoxConfig = {
    maxRetries: number;
    inner: PolicyNodeIndex;
  };
  export type TimeBoxConfig = {
    limit: Duration;
    inner: PolicyNodeIndex;
  };
  export type ClampConfig = {
    minDelay: Duration;
    maxDelay: Duration;
    inner: PolicyNodeIndex;
  };
  export type AddDelayConfig = {
    delay: Duration;
    inner: PolicyNodeIndex;
  };
  export type JitterConfig = {
    factor: number;
    inner: PolicyNodeIndex;
  };
  /**
   * filtered-config embeds a full retry-predicate inline (self-contained)
   */
  export type FilteredConfig = {
    predicate: RetryPredicate;
    inner: PolicyNodeIndex;
  };
  export type PolicyNode = 
  {
    tag: 'periodic'
    val: Duration
  } |
  {
    tag: 'exponential'
    val: ExponentialConfig
  } |
  {
    tag: 'fibonacci'
    val: FibonacciConfig
  } |
  {
    tag: 'immediate'
  } |
  {
    tag: 'never'
  } |
  {
    tag: 'count-box'
    val: CountBoxConfig
  } |
  {
    tag: 'time-box'
    val: TimeBoxConfig
  } |
  {
    tag: 'clamp-delay'
    val: ClampConfig
  } |
  {
    tag: 'add-delay'
    val: AddDelayConfig
  } |
  {
    tag: 'jitter'
    val: JitterConfig
  } |
  {
    tag: 'filtered-on'
    val: FilteredConfig
  } |
  {
    tag: 'and-then'
    val: [PolicyNodeIndex, PolicyNodeIndex]
  } |
  {
    tag: 'policy-union'
    val: [PolicyNodeIndex, PolicyNodeIndex]
  } |
  {
    tag: 'policy-intersect'
    val: [PolicyNodeIndex, PolicyNodeIndex]
  };
  /**
   * A composable retry policy tree.
   * Root is nodes[0]. Children referenced by policy-node-index.
   */
  export type RetryPolicy = {
    nodes: PolicyNode[];
  };
  /**
   * ── Named policy rule ────────────────────────────────────────────
   * A named retry policy rule: predicate selects when it applies,
   * policy defines the retry strategy, priority controls evaluation order
   * (higher = checked first).
   */
  export type NamedRetryPolicy = {
    name: string;
    priority: number;
    predicate: RetryPredicate;
    policy: RetryPolicy;
  };
}
