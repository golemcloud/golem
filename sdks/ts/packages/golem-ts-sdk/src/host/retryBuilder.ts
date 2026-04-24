// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import type {
  NamedRetryPolicy as RawNamedRetryPolicy,
  PolicyNode as RawPolicyNode,
  PredicateNode as RawPredicateNode,
  PredicateValue as RawPredicateValue,
  RetryPolicy as RawRetryPolicy,
  RetryPredicate as RawRetryPredicate,
} from 'golem:api/retry@1.5.0';
import type { Duration as RawDuration } from 'wasi:clocks/monotonic-clock@0.2.3';

const INT64_MIN = -(1n << 63n);
const INT64_MAX = (1n << 63n) - 1n;
const UINT32_MAX = 0xffff_ffff;
const UINT64_MAX = (1n << 64n) - 1n;
const DURATION_BRAND = Symbol.for('golem.retry.duration');
const NAMED_POLICY_BRAND = Symbol.for('golem.retry.named-policy');

type PredicateNodeDef =
  | { tag: 'pred-true' }
  | { tag: 'pred-false' }
  | {
      tag: 'prop-eq' | 'prop-neq' | 'prop-gt' | 'prop-gte' | 'prop-lt' | 'prop-lte';
      property: string;
      value: PredicateValueInput;
    }
  | { tag: 'prop-exists'; property: string }
  | { tag: 'prop-in'; property: string; values: readonly PredicateValueInput[] }
  | {
      tag: 'prop-matches' | 'prop-starts-with' | 'prop-contains';
      property: string;
      pattern: string;
    }
  | {
      tag: 'pred-and' | 'pred-or';
      left: PredicateNodeDef;
      right: PredicateNodeDef;
    }
  | { tag: 'pred-not'; inner: PredicateNodeDef };

type PolicyNodeDef =
  | { tag: 'immediate' }
  | { tag: 'never' }
  | { tag: 'periodic'; delay: DurationInput }
  | { tag: 'exponential'; baseDelay: DurationInput; factor: number }
  | { tag: 'fibonacci'; first: DurationInput; second: DurationInput }
  | { tag: 'count-box'; maxRetries: number; inner: PolicyNodeDef }
  | { tag: 'time-box'; limit: DurationInput; inner: PolicyNodeDef }
  | {
      tag: 'clamp-delay';
      minDelay: DurationInput;
      maxDelay: DurationInput;
      inner: PolicyNodeDef;
    }
  | { tag: 'add-delay'; delay: DurationInput; inner: PolicyNodeDef }
  | { tag: 'jitter'; factor: number; inner: PolicyNodeDef }
  | { tag: 'filtered-on'; predicate: PredicateNodeDef; inner: PolicyNodeDef }
  | {
      tag: 'and-then' | 'policy-union' | 'policy-intersect';
      left: PolicyNodeDef;
      right: PolicyNodeDef;
    };

export type PredicateValueInput = string | boolean | bigint | number;
export type DurationInput = Duration | RawDuration | bigint | number;
export type NamedPolicyInput = NamedPolicy | RawNamedRetryPolicy;

/**
 * A validated retry duration helper backed by raw nanoseconds.
 */
export class Duration {
  readonly [DURATION_BRAND] = true;

  private constructor(private readonly nanoseconds: bigint) {}

  static nanoseconds(value: bigint | number): Duration {
    return new Duration(toRawDuration(value, 'Duration.nanoseconds'));
  }

  static microseconds(value: bigint | number): Duration {
    return new Duration(scaleDurationUnit(value, 1_000n, 'Duration.microseconds'));
  }

  static milliseconds(value: bigint | number): Duration {
    return new Duration(scaleDurationUnit(value, 1_000_000n, 'Duration.milliseconds'));
  }

  static seconds(value: bigint | number): Duration {
    return new Duration(scaleDurationUnit(value, 1_000_000_000n, 'Duration.seconds'));
  }

  static minutes(value: bigint | number): Duration {
    return new Duration(scaleDurationUnit(value, 60_000_000_000n, 'Duration.minutes'));
  }

  static hours(value: bigint | number): Duration {
    return new Duration(scaleDurationUnit(value, 3_600_000_000_000n, 'Duration.hours'));
  }

  static from(value: DurationInput): Duration {
    return value instanceof Duration ? value : new Duration(toRawDuration(value, 'Duration.from'));
  }

  toRaw(): RawDuration {
    return this.nanoseconds;
  }

  asNanoseconds(): bigint {
    return this.nanoseconds;
  }
}

/**
 * Retry property keys and helpers that match the platform retry context vocabulary.
 */
export class Props {
  static readonly verb = 'verb';
  static readonly nounUri = 'noun-uri';
  static readonly uriScheme = 'uri-scheme';
  static readonly uriHost = 'uri-host';
  static readonly uriPort = 'uri-port';
  static readonly uriPath = 'uri-path';
  static readonly statusCode = 'status-code';
  static readonly errorType = 'error-type';
  static readonly function = 'function';
  static readonly targetComponentId = 'target-component-id';
  static readonly targetAgentType = 'target-agent-type';
  static readonly dbType = 'db-type';
  static readonly trapType = 'trap-type';

  static entry(property: string, value: PredicateValueInput): [string, RawPredicateValue] {
    return [property, toRawPredicateValue(value, `Props.entry(${property})`)];
  }

  static entries(
    properties: Readonly<Record<string, PredicateValueInput>>,
  ): [string, RawPredicateValue][] {
    return Object.entries(properties).map(([property, value]) => Props.entry(property, value));
  }
}

/**
 * Immutable predicate builder compiled to the flattened WIT retry-predicate format.
 */
export class Predicate {
  private constructor(private readonly node: PredicateNodeDef) {}

  static always(): Predicate {
    return new Predicate({ tag: 'pred-true' });
  }

  static never(): Predicate {
    return new Predicate({ tag: 'pred-false' });
  }

  static eq(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-eq', property, value });
  }

  static neq(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-neq', property, value });
  }

  static gt(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-gt', property, value });
  }

  static gte(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-gte', property, value });
  }

  static lt(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-lt', property, value });
  }

  static lte(property: string, value: PredicateValueInput): Predicate {
    return new Predicate({ tag: 'prop-lte', property, value });
  }

  static exists(property: string): Predicate {
    return new Predicate({ tag: 'prop-exists', property });
  }

  static oneOf(property: string, values: readonly PredicateValueInput[]): Predicate {
    return new Predicate({ tag: 'prop-in', property, values });
  }

  static matchesGlob(property: string, pattern: string): Predicate {
    return new Predicate({ tag: 'prop-matches', property, pattern });
  }

  static startsWith(property: string, prefix: string): Predicate {
    return new Predicate({ tag: 'prop-starts-with', property, pattern: prefix });
  }

  static contains(property: string, substring: string): Predicate {
    return new Predicate({ tag: 'prop-contains', property, pattern: substring });
  }

  and(other: Predicate): Predicate {
    return new Predicate({ tag: 'pred-and', left: this.node, right: other.node });
  }

  or(other: Predicate): Predicate {
    return new Predicate({ tag: 'pred-or', left: this.node, right: other.node });
  }

  not(): Predicate {
    return new Predicate({ tag: 'pred-not', inner: this.node });
  }

  toNode(): PredicateNodeDef {
    return this.node;
  }

  toRaw(): RawRetryPredicate {
    return buildRawPredicate(this.node);
  }
}

/**
 * Immutable retry policy builder compiled to the flattened WIT retry-policy format.
 */
export class Policy {
  private constructor(private readonly node: PolicyNodeDef) {}

  static immediate(): Policy {
    return new Policy({ tag: 'immediate' });
  }

  static never(): Policy {
    return new Policy({ tag: 'never' });
  }

  static periodic(delay: DurationInput): Policy {
    return new Policy({ tag: 'periodic', delay });
  }

  static exponential(baseDelay: DurationInput, factor: number): Policy {
    return new Policy({ tag: 'exponential', baseDelay, factor });
  }

  static fibonacci(first: DurationInput, second: DurationInput): Policy {
    return new Policy({ tag: 'fibonacci', first, second });
  }

  maxRetries(maxRetries: number): Policy {
    return new Policy({ tag: 'count-box', maxRetries, inner: this.node });
  }

  within(limit: DurationInput): Policy {
    return new Policy({ tag: 'time-box', limit, inner: this.node });
  }

  clamp(minDelay: DurationInput, maxDelay: DurationInput): Policy {
    return new Policy({ tag: 'clamp-delay', minDelay, maxDelay, inner: this.node });
  }

  addDelay(delay: DurationInput): Policy {
    return new Policy({ tag: 'add-delay', delay, inner: this.node });
  }

  withJitter(factor: number): Policy {
    return new Policy({ tag: 'jitter', factor, inner: this.node });
  }

  onlyWhen(predicate: Predicate): Policy {
    return new Policy({ tag: 'filtered-on', predicate: predicate.toNode(), inner: this.node });
  }

  andThen(other: Policy): Policy {
    return new Policy({ tag: 'and-then', left: this.node, right: other.node });
  }

  union(other: Policy): Policy {
    return new Policy({ tag: 'policy-union', left: this.node, right: other.node });
  }

  intersect(other: Policy): Policy {
    return new Policy({ tag: 'policy-intersect', left: this.node, right: other.node });
  }

  toRaw(): RawRetryPolicy {
    return buildRawPolicy(this.node);
  }
}

/**
 * Immutable named retry policy builder with sensible defaults for priority and predicate.
 */
export class NamedPolicy {
  readonly [NAMED_POLICY_BRAND] = true;

  private constructor(
    readonly name: string,
    private readonly policy: Policy,
    private readonly predicate: Predicate,
    private readonly policyPriority: number,
  ) {}

  static named(name: string, policy: Policy): NamedPolicy {
    return new NamedPolicy(name, policy, Predicate.always(), 0);
  }

  priority(priority: number): NamedPolicy {
    return new NamedPolicy(this.name, this.policy, this.predicate, priority);
  }

  appliesWhen(predicate: Predicate): NamedPolicy {
    return new NamedPolicy(this.name, this.policy, predicate, this.policyPriority);
  }

  toRaw(): RawNamedRetryPolicy {
    return {
      name: this.name,
      priority: ensureUint32(this.policyPriority, 'NamedPolicy.priority'),
      predicate: this.predicate.toRaw(),
      policy: this.policy.toRaw(),
    };
  }
}

export function toRawPredicateValue(
  value: PredicateValueInput,
  context = 'predicate value',
): RawPredicateValue {
  if (typeof value === 'string') {
    return { tag: 'text', val: value };
  }

  if (typeof value === 'boolean') {
    return { tag: 'boolean', val: value };
  }

  const integer = toInt64(value, context);
  return { tag: 'integer', val: integer };
}

export function toRawDuration(value: DurationInput, context = 'duration'): RawDuration {
  if (isDuration(value)) {
    return value.toRaw();
  }

  const nanoseconds = toBigIntInteger(value, context);
  if (nanoseconds < 0n) {
    throw new Error(`${context} must be non-negative`);
  }
  if (nanoseconds > UINT64_MAX) {
    throw new Error(`${context} must fit an unsigned 64-bit duration`);
  }

  return nanoseconds;
}

export function toRawPredicate(predicate: Predicate): RawRetryPredicate {
  return predicate.toRaw();
}

export function toRawPolicy(policy: Policy): RawRetryPolicy {
  return policy.toRaw();
}

export function toRawNamedPolicy(policy: NamedPolicyInput): RawNamedRetryPolicy {
  return isNamedPolicy(policy) ? policy.toRaw() : policy;
}

function isDuration(value: DurationInput): value is Duration {
  const candidate = value as unknown as {
    [DURATION_BRAND]?: unknown;
    toRaw?: unknown;
  };

  return (
    typeof value === 'object' &&
    value !== null &&
    candidate[DURATION_BRAND] === true &&
    typeof candidate.toRaw === 'function'
  );
}

function isNamedPolicy(policy: NamedPolicyInput): policy is NamedPolicy {
  const candidate = policy as unknown as {
    [NAMED_POLICY_BRAND]?: unknown;
    toRaw?: unknown;
  };

  return (
    typeof policy === 'object' &&
    policy !== null &&
    candidate[NAMED_POLICY_BRAND] === true &&
    typeof candidate.toRaw === 'function'
  );
}

function buildRawPredicate(root: PredicateNodeDef): RawRetryPredicate {
  const nodes: RawPredicateNode[] = [];
  appendPredicateNode(root, nodes);
  return { nodes };
}

function appendPredicateNode(node: PredicateNodeDef, nodes: RawPredicateNode[]): number {
  const index = nodes.length;
  nodes.push({ tag: 'pred-false' });

  switch (node.tag) {
    case 'pred-true':
    case 'pred-false':
      nodes[index] = { tag: node.tag };
      return index;
    case 'prop-eq':
    case 'prop-neq':
    case 'prop-gt':
    case 'prop-gte':
    case 'prop-lt':
    case 'prop-lte':
      nodes[index] = {
        tag: node.tag,
        val: {
          propertyName: node.property,
          value: toRawPredicateValue(node.value, `Predicate.${node.tag}(${node.property})`),
        },
      };
      return index;
    case 'prop-exists':
      nodes[index] = { tag: 'prop-exists', val: node.property };
      return index;
    case 'prop-in':
      nodes[index] = {
        tag: 'prop-in',
        val: {
          propertyName: node.property,
          values: node.values.map((value, valueIndex) =>
            toRawPredicateValue(value, `Predicate.oneOf(${node.property})[${valueIndex}]`),
          ),
        },
      };
      return index;
    case 'prop-matches':
    case 'prop-starts-with':
    case 'prop-contains':
      nodes[index] = {
        tag: node.tag,
        val: {
          propertyName: node.property,
          pattern: node.pattern,
        },
      };
      return index;
    case 'pred-and':
    case 'pred-or': {
      const left = appendPredicateNode(node.left, nodes);
      const right = appendPredicateNode(node.right, nodes);
      nodes[index] = { tag: node.tag, val: [left, right] };
      return index;
    }
    case 'pred-not': {
      const inner = appendPredicateNode(node.inner, nodes);
      nodes[index] = { tag: 'pred-not', val: inner };
      return index;
    }
  }
}

function buildRawPolicy(root: PolicyNodeDef): RawRetryPolicy {
  const nodes: RawPolicyNode[] = [];
  appendPolicyNode(root, nodes);
  return { nodes };
}

function appendPolicyNode(node: PolicyNodeDef, nodes: RawPolicyNode[]): number {
  const index = nodes.length;
  nodes.push({ tag: 'never' });

  switch (node.tag) {
    case 'immediate':
    case 'never':
      nodes[index] = { tag: node.tag };
      return index;
    case 'periodic':
      nodes[index] = {
        tag: 'periodic',
        val: toRawDuration(node.delay, 'Policy.periodic'),
      };
      return index;
    case 'exponential':
      nodes[index] = {
        tag: 'exponential',
        val: {
          baseDelay: toRawDuration(node.baseDelay, 'Policy.exponential.baseDelay'),
          factor: ensureFinitePositiveNumber(node.factor, 'Policy.exponential.factor'),
        },
      };
      return index;
    case 'fibonacci':
      nodes[index] = {
        tag: 'fibonacci',
        val: {
          first: toRawDuration(node.first, 'Policy.fibonacci.first'),
          second: toRawDuration(node.second, 'Policy.fibonacci.second'),
        },
      };
      return index;
    case 'count-box': {
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'count-box',
        val: {
          maxRetries: ensureUint32(node.maxRetries, 'Policy.maxRetries'),
          inner,
        },
      };
      return index;
    }
    case 'time-box': {
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'time-box',
        val: {
          limit: toRawDuration(node.limit, 'Policy.within.limit'),
          inner,
        },
      };
      return index;
    }
    case 'clamp-delay': {
      const minDelay = toRawDuration(node.minDelay, 'Policy.clamp.minDelay');
      const maxDelay = toRawDuration(node.maxDelay, 'Policy.clamp.maxDelay');
      if (minDelay > maxDelay) {
        throw new Error('Policy.clamp requires minDelay to be less than or equal to maxDelay');
      }
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'clamp-delay',
        val: {
          minDelay,
          maxDelay,
          inner,
        },
      };
      return index;
    }
    case 'add-delay': {
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'add-delay',
        val: {
          delay: toRawDuration(node.delay, 'Policy.addDelay.delay'),
          inner,
        },
      };
      return index;
    }
    case 'jitter': {
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'jitter',
        val: {
          factor: ensureFiniteNonNegativeNumber(node.factor, 'Policy.withJitter.factor'),
          inner,
        },
      };
      return index;
    }
    case 'filtered-on': {
      const inner = appendPolicyNode(node.inner, nodes);
      nodes[index] = {
        tag: 'filtered-on',
        val: {
          predicate: buildRawPredicate(node.predicate),
          inner,
        },
      };
      return index;
    }
    case 'and-then':
    case 'policy-union':
    case 'policy-intersect': {
      const left = appendPolicyNode(node.left, nodes);
      const right = appendPolicyNode(node.right, nodes);
      nodes[index] = { tag: node.tag, val: [left, right] };
      return index;
    }
  }
}

function scaleDurationUnit(
  value: bigint | number,
  multiplier: bigint,
  context: string,
): RawDuration {
  const base = toBigIntInteger(value, context);
  if (base < 0n) {
    throw new Error(`${context} must be non-negative`);
  }

  const scaled = base * multiplier;
  if (scaled > UINT64_MAX) {
    throw new Error(`${context} must fit an unsigned 64-bit duration`);
  }

  return scaled;
}

function toInt64(value: bigint | number, context: string): bigint {
  const integer = toBigIntInteger(value, context);
  if (integer < INT64_MIN || integer > INT64_MAX) {
    throw new Error(`${context} must fit a signed 64-bit integer`);
  }
  return integer;
}

function toBigIntInteger(value: bigint | number, context: string): bigint {
  if (typeof value === 'bigint') {
    return value;
  }

  if (!Number.isSafeInteger(value)) {
    throw new Error(`${context} must be a safe integer number or bigint`);
  }

  return BigInt(value);
}

function ensureUint32(value: number, context: string): number {
  if (!Number.isSafeInteger(value) || value < 0 || value > UINT32_MAX) {
    throw new Error(`${context} must be a non-negative 32-bit integer`);
  }

  return value;
}

function ensureFinitePositiveNumber(value: number, context: string): number {
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${context} must be a finite number greater than 0`);
  }

  return value;
}

function ensureFiniteNonNegativeNumber(value: number, context: string): number {
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`${context} must be a finite number greater than or equal to 0`);
  }

  return value;
}
