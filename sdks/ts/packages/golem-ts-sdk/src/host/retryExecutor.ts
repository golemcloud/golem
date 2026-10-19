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

import type { PolicyNode, PredicateNode, PredicateValue, RetryPolicy } from 'golem:api/retry@1.5.0';
import { awaitAbortable, throwIfAborted } from '../internal/pollableUtils';
import {
  toRawPolicy,
  toRawPredicateValue,
  type PolicyInput,
  type PredicateValueInput,
} from './retryBuilder';

const UINT32_MAX = 0xffff_ffff;
const UINT64_MAX = (1n << 64n) - 1n;
const DURATION_MAX = UINT64_MAX * 1_000_000_000n + 999_999_999n;
const MAX_TIMER_DELAY_MS = 0x7fff_ffff;
const MAX_RAW_NODES = 4_096;
const MAX_COMPILED_NODES = 4_096;
const MAX_COMPILED_PAYLOAD_BYTES = 1_048_576;
const MAX_POLICY_DEPTH = 256;

export type RetryProperties =
  | Readonly<Record<string, PredicateValueInput>>
  | Iterable<readonly [string, PredicateValueInput]>;

export interface RetryOptions {
  /** Projects semantic retry properties from each thrown or rejected value. */
  properties?: (error: unknown, retryCount: number) => RetryProperties;
  /** Cancels an in-flight attempt or delay. The underlying user operation must observe it to stop. */
  signal?: AbortSignal;
}

/** An invalid policy AST or a predicate that cannot be evaluated against projected properties. */
export class RetryPolicyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RetryPolicyError';
  }
}

type Properties = Map<string, PredicateValue>;

type CompiledPredicate =
  | { tag: 'pred-true' | 'pred-false' }
  | { tag: 'prop-exists'; property: string }
  | {
      tag: 'prop-eq' | 'prop-neq' | 'prop-gt' | 'prop-gte' | 'prop-lt' | 'prop-lte';
      property: string;
      value: PredicateValue;
    }
  | { tag: 'prop-in'; property: string; values: PredicateValue[] }
  | {
      tag: 'prop-matches' | 'prop-starts-with' | 'prop-contains';
      property: string;
      pattern: string;
    }
  | { tag: 'pred-and' | 'pred-or'; left: CompiledPredicate; right: CompiledPredicate }
  | { tag: 'pred-not'; inner: CompiledPredicate };

type CompiledPolicy =
  | { tag: 'periodic'; delay: bigint }
  | { tag: 'exponential'; baseDelay: bigint; factor: number }
  | { tag: 'fibonacci'; first: bigint; second: bigint }
  | { tag: 'immediate' | 'never' }
  | { tag: 'count-box'; maxRetries: number; inner: CompiledPolicy }
  | { tag: 'time-box'; limit: bigint; inner: CompiledPolicy }
  | { tag: 'clamp-delay'; minDelay: bigint; maxDelay: bigint; inner: CompiledPolicy }
  | { tag: 'add-delay'; delay: bigint; inner: CompiledPolicy }
  | { tag: 'jitter'; factor: number; inner: CompiledPolicy }
  | { tag: 'filtered-on'; predicate: CompiledPredicate; inner: CompiledPolicy }
  | {
      tag: 'and-then' | 'policy-union' | 'policy-intersect';
      left: CompiledPolicy;
      right: CompiledPolicy;
    };

type PolicyState =
  | { tag: 'counter'; count: number }
  | { tag: 'terminal' }
  | { tag: 'wrapper'; inner: PolicyState }
  | { tag: 'count-box'; attempts: number; inner: PolicyState }
  | { tag: 'and-then'; left: PolicyState; right: PolicyState; onRight: boolean }
  | { tag: 'pair'; left: PolicyState; right: PolicyState };

type Verdict = { tag: 'retry'; delay: bigint } | { tag: 'give-up' };

interface Compilation<T> {
  value: T;
  nodes: number;
  payloadBytes: number;
  height: number;
}

interface Compiler {
  policyCache: Map<number, Compilation<CompiledPolicy>>;
  policyVisiting: Set<number>;
  predicateCaches: WeakMap<object, Map<number, Compilation<CompiledPredicate>>>;
  predicateVisiting: WeakMap<object, Set<number>>;
  registeredPredicates: WeakSet<object>;
  rawNodes: number;
  allocatedPayloadBytes: number;
}

/**
 * Runs arbitrary user code using a Golem semantic retry policy entirely in user space.
 *
 * The callback receives a zero-based physical attempt number. Its return value may be
 * synchronous or Promise-like; this function always returns a Promise. These retries
 * are ordinary user-space calls, not executor-managed retry attempts, and do not form
 * one durable retry sequence across suspension or recovery.
 *
 * Local `prop-matches` predicates translate `*` to JavaScript regex `.*` and `?` to `.`,
 * without flags. Use a host-driven retry policy when full platform glob semantics are required.
 */
export async function retry<T>(
  policy: PolicyInput,
  operation: (attempt: number) => T | PromiseLike<T>,
  options: RetryOptions = {},
): Promise<T> {
  const compiled = compilePolicy(toRawPolicy(policy));
  let state = initialState(compiled);
  let retryCount = 0;
  const startedAt = monotonicMilliseconds();

  while (true) {
    throwIfAborted(options.signal);

    try {
      const attempt = Promise.resolve().then(() => {
        throwIfAborted(options.signal);
        return operation(retryCount);
      });
      return await awaitAbortable(attempt, options.signal);
    } catch (error) {
      throwIfAborted(options.signal);
      const properties = projectProperties(options.properties?.(error, retryCount));
      const elapsed = elapsedNanoseconds(startedAt);
      const step = evaluate(compiled, state, elapsed, properties);
      state = step.state;

      if (step.verdict.tag === 'give-up') {
        throw error;
      }

      retryCount += 1;
      await sleep(step.verdict.delay, options.signal);
    }
  }
}

function compilePolicy(policy: RetryPolicy): CompiledPolicy {
  if (!isObject(policy) || !Array.isArray(policy.nodes) || policy.nodes.length === 0) {
    throw invalid('policy must contain a root node at index 0');
  }

  const compiler: Compiler = {
    policyCache: new Map(),
    policyVisiting: new Set(),
    predicateCaches: new WeakMap(),
    predicateVisiting: new WeakMap(),
    registeredPredicates: new WeakSet(),
    rawNodes: policy.nodes.length,
    allocatedPayloadBytes: 0,
  };
  if (compiler.rawNodes > MAX_RAW_NODES) throw tooComplex();
  for (const node of policy.nodes) {
    if (isObject(node) && node.tag === 'filtered-on' && isObject(node.val)) {
      registerRawPredicate(compiler, node.val.predicate);
    }
  }

  return compilePolicyNode(compiler, policy.nodes, 0, 0).value;
}

function compilePolicyNode(
  compiler: Compiler,
  nodes: PolicyNode[],
  index: number,
  depth: number,
): Compilation<CompiledPolicy> {
  const node = indexedNode(nodes, index, 'policy');
  if (compiler.policyVisiting.has(index)) throw invalid(`policy contains a cycle at node ${index}`);
  const cached = compiler.policyCache.get(index);
  if (cached !== undefined) {
    ensureDepth(depth, cached.height);
    return cached;
  }
  ensureDepth(depth, 0);
  compiler.policyVisiting.add(index);
  const inner = (child: unknown) =>
    compilePolicyNode(compiler, nodes, validIndex(child, 'policy child'), depth + 1);
  const pair = (value: unknown): [Compilation<CompiledPolicy>, Compilation<CompiledPolicy>] => {
    const [left, right] = validPair(value, 'policy children');
    return [inner(left), inner(right)];
  };

  let result: Compilation<CompiledPolicy>;
  switch (node.tag) {
    case 'immediate':
    case 'never':
      result = leaf({ tag: node.tag });
      break;
    case 'periodic':
      result = leaf({ tag: node.tag, delay: validDuration(node.val, 'periodic delay') });
      break;
    case 'exponential': {
      const value = validObject(node.val, 'exponential config');
      result = leaf({
        tag: node.tag,
        baseDelay: validDuration(value.baseDelay, 'exponential base delay'),
        factor: validPositiveFactor(value.factor, 'exponential factor'),
      });
      break;
    }
    case 'fibonacci': {
      const value = validObject(node.val, 'fibonacci config');
      result = leaf({
        tag: node.tag,
        first: validDuration(value.first, 'fibonacci first delay'),
        second: validDuration(value.second, 'fibonacci second delay'),
      });
      break;
    }
    case 'count-box': {
      const value = validObject(node.val, 'count-box config');
      const compiledInner = inner(value.inner);
      result = parent(
        {
          tag: node.tag,
          maxRetries: validUint32(value.maxRetries, 'count-box max retries'),
          inner: compiledInner.value,
        },
        compiledInner,
      );
      break;
    }
    case 'time-box': {
      const value = validObject(node.val, 'time-box config');
      const compiledInner = inner(value.inner);
      result = parent(
        {
          tag: node.tag,
          limit: validDuration(value.limit, 'time-box limit'),
          inner: compiledInner.value,
        },
        compiledInner,
      );
      break;
    }
    case 'clamp-delay': {
      const value = validObject(node.val, 'clamp-delay config');
      const minDelay = validDuration(value.minDelay, 'clamp minimum delay');
      const maxDelay = validDuration(value.maxDelay, 'clamp maximum delay');
      if (minDelay > maxDelay) throw invalid('clamp minimum delay must not exceed maximum delay');
      const compiledInner = inner(value.inner);
      result = parent(
        { tag: node.tag, minDelay, maxDelay, inner: compiledInner.value },
        compiledInner,
      );
      break;
    }
    case 'add-delay': {
      const value = validObject(node.val, 'add-delay config');
      const compiledInner = inner(value.inner);
      result = parent(
        {
          tag: node.tag,
          delay: validDuration(value.delay, 'additional delay'),
          inner: compiledInner.value,
        },
        compiledInner,
      );
      break;
    }
    case 'jitter': {
      const value = validObject(node.val, 'jitter config');
      const compiledInner = inner(value.inner);
      result = parent(
        {
          tag: node.tag,
          factor: validNonNegativeFactor(value.factor, 'jitter factor'),
          inner: compiledInner.value,
        },
        compiledInner,
      );
      break;
    }
    case 'filtered-on': {
      const value = validObject(node.val, 'filtered-on config');
      const predicate = compilePredicate(compiler, value.predicate, depth + 1);
      const compiledInner = inner(value.inner);
      result = combineCompilation(
        {
          tag: node.tag,
          predicate: predicate.value,
          inner: compiledInner.value,
        },
        predicate,
        compiledInner,
      );
      break;
    }
    case 'and-then':
    case 'policy-union':
    case 'policy-intersect': {
      const [left, right] = pair(node.val);
      result = combineCompilation(
        { tag: node.tag, left: left.value, right: right.value },
        left,
        right,
      );
      break;
    }
    default:
      throw invalid(`unsupported policy node '${String((node as { tag?: unknown }).tag)}'`);
  }
  compiler.policyVisiting.delete(index);
  ensureComplexity(result);
  compiler.policyCache.set(index, result);
  return result;
}

function compilePredicate(
  compiler: Compiler,
  predicate: unknown,
  depth: number,
): Compilation<CompiledPredicate> {
  if (!isObject(predicate) || !Array.isArray(predicate.nodes) || predicate.nodes.length === 0) {
    throw invalid('predicate must contain a root node at index 0');
  }
  registerRawPredicate(compiler, predicate);
  let cache = compiler.predicateCaches.get(predicate);
  let visiting = compiler.predicateVisiting.get(predicate);
  if (cache === undefined || visiting === undefined) {
    cache = new Map();
    visiting = new Set();
    compiler.predicateCaches.set(predicate, cache);
    compiler.predicateVisiting.set(predicate, visiting);
  }
  return compilePredicateNode(
    compiler,
    predicate.nodes as PredicateNode[],
    0,
    depth,
    cache,
    visiting,
  );
}

function compilePredicateNode(
  compiler: Compiler,
  nodes: PredicateNode[],
  index: number,
  depth: number,
  cache: Map<number, Compilation<CompiledPredicate>>,
  visiting: Set<number>,
): Compilation<CompiledPredicate> {
  const node = indexedNode(nodes, index, 'predicate');
  if (visiting.has(index)) throw invalid(`predicate contains a cycle at node ${index}`);
  const cached = cache.get(index);
  if (cached !== undefined) {
    ensureDepth(depth, cached.height);
    return cached;
  }
  ensureDepth(depth, 0);
  visiting.add(index);
  const inner = (child: unknown) =>
    compilePredicateNode(
      compiler,
      nodes,
      validIndex(child, 'predicate child'),
      depth + 1,
      cache,
      visiting,
    );

  let result: Compilation<CompiledPredicate>;
  switch (node.tag) {
    case 'pred-true':
    case 'pred-false':
      result = leaf({ tag: node.tag });
      break;
    case 'prop-exists': {
      const property = validString(node.val, 'property name');
      const payloadBytes = stringBytes(property);
      chargePayload(compiler, payloadBytes);
      result = leaf({ tag: node.tag, property }, payloadBytes);
      break;
    }
    case 'prop-eq':
    case 'prop-neq':
    case 'prop-gt':
    case 'prop-gte':
    case 'prop-lt':
    case 'prop-lte': {
      const value = validObject(node.val, `${node.tag} config`);
      const property = validString(value.propertyName, 'property name');
      const payloadBytes = saturatingNumberAdd(
        stringBytes(property),
        predicateValueInputBytes(value.value),
      );
      chargePayload(compiler, payloadBytes);
      const predicateValue = validPredicateValue(value.value);
      result = leaf(
        {
          tag: node.tag,
          property,
          value: predicateValue,
        },
        payloadBytes,
      );
      break;
    }
    case 'prop-in': {
      const value = validObject(node.val, 'prop-in config');
      if (!Array.isArray(value.values)) throw invalid('prop-in values must be an array');
      const property = validString(value.propertyName, 'property name');
      let payloadBytes = stringBytes(property);
      chargePayload(compiler, payloadBytes);
      const values: PredicateValue[] = [];
      for (const item of value.values) {
        const itemBytes = saturatingNumberAdd(16, predicateValueInputBytes(item));
        chargePayload(compiler, itemBytes);
        const predicateValue = validPredicateValue(item);
        payloadBytes = saturatingNumberAdd(payloadBytes, itemBytes);
        values.push(predicateValue);
      }
      result = leaf(
        {
          tag: node.tag,
          property,
          values,
        },
        payloadBytes,
      );
      break;
    }
    case 'prop-matches':
    case 'prop-starts-with':
    case 'prop-contains': {
      const value = validObject(node.val, `${node.tag} config`);
      const property = validString(value.propertyName, 'property name');
      const pattern = validString(value.pattern, 'property pattern');
      const payloadBytes = saturatingNumberAdd(stringBytes(property), stringBytes(pattern));
      chargePayload(compiler, payloadBytes);
      result = leaf(
        {
          tag: node.tag,
          property,
          pattern,
        },
        payloadBytes,
      );
      break;
    }
    case 'pred-and':
    case 'pred-or': {
      const [left, right] = validPair(node.val, 'predicate children');
      const compiledLeft = inner(left);
      const compiledRight = inner(right);
      result = combineCompilation(
        { tag: node.tag, left: compiledLeft.value, right: compiledRight.value },
        compiledLeft,
        compiledRight,
      );
      break;
    }
    case 'pred-not': {
      const compiledInner = inner(node.val);
      result = parent({ tag: node.tag, inner: compiledInner.value }, compiledInner);
      break;
    }
    default:
      throw invalid(`unsupported predicate node '${String((node as { tag?: unknown }).tag)}'`);
  }
  visiting.delete(index);
  ensureComplexity(result);
  cache.set(index, result);
  return result;
}

function registerRawPredicate(compiler: Compiler, predicate: unknown): void {
  if (!isObject(predicate) || compiler.registeredPredicates.has(predicate)) return;
  compiler.registeredPredicates.add(predicate);
  if (Array.isArray(predicate.nodes)) {
    compiler.rawNodes = saturatingNumberAdd(compiler.rawNodes, predicate.nodes.length);
    if (compiler.rawNodes > MAX_RAW_NODES) throw tooComplex();
  }
}

function leaf<T>(value: T, payloadBytes = 0): Compilation<T> {
  return { value, nodes: 1, payloadBytes, height: 0 };
}

function parent<T, U>(value: T, child: Compilation<U>): Compilation<T> {
  return {
    value,
    nodes: saturatingNumberAdd(1, child.nodes),
    payloadBytes: child.payloadBytes,
    height: child.height + 1,
  };
}

function combineCompilation<T, U, V>(
  value: T,
  left: Compilation<U>,
  right: Compilation<V>,
): Compilation<T> {
  return {
    value,
    nodes: saturatingNumberAdd(1, saturatingNumberAdd(left.nodes, right.nodes)),
    payloadBytes: saturatingNumberAdd(left.payloadBytes, right.payloadBytes),
    height: Math.max(left.height, right.height) + 1,
  };
}

function ensureDepth(depth: number, height: number): void {
  if (depth + height > MAX_POLICY_DEPTH) throw tooComplex();
}

function ensureComplexity(compilation: Compilation<unknown>): void {
  ensureDepth(0, compilation.height);
  if (
    compilation.nodes > MAX_COMPILED_NODES ||
    compilation.payloadBytes > MAX_COMPILED_PAYLOAD_BYTES
  ) {
    throw tooComplex();
  }
}

function chargePayload(compiler: Compiler, bytes: number): void {
  compiler.allocatedPayloadBytes = saturatingNumberAdd(compiler.allocatedPayloadBytes, bytes);
  if (compiler.allocatedPayloadBytes > MAX_COMPILED_PAYLOAD_BYTES) throw tooComplex();
}

function predicateValueInputBytes(value: unknown): number {
  if (!isObject(value)) throw invalid('predicate value must be a tagged value');
  if (value.tag === 'text' && typeof value.val === 'string') return stringBytes(value.val);
  if (value.tag === 'boolean' && typeof value.val === 'boolean') return 0;
  if (
    value.tag === 'integer' &&
    typeof value.val === 'bigint' &&
    value.val >= -(1n << 63n) &&
    value.val <= (1n << 63n) - 1n
  ) {
    return 0;
  }
  throw invalid('predicate value does not match its WIT tag');
}

function stringBytes(value: string): number {
  let bytes = 0;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        bytes += 4;
        index += 1;
      } else {
        bytes += 3;
      }
    } else {
      bytes += 3;
    }
    if (bytes > MAX_COMPILED_PAYLOAD_BYTES) return bytes;
  }
  return bytes;
}

function saturatingNumberAdd(left: number, right: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, left + right);
}

function tooComplex(): RetryPolicyError {
  return invalid('policy is too deeply nested or expansive');
}

function initialState(policy: CompiledPolicy): PolicyState {
  switch (policy.tag) {
    case 'periodic':
    case 'exponential':
    case 'fibonacci':
    case 'immediate':
      return { tag: 'counter', count: 0 };
    case 'never':
      return { tag: 'terminal' };
    case 'count-box':
      return { tag: 'count-box', attempts: 0, inner: initialState(policy.inner) };
    case 'time-box':
    case 'clamp-delay':
    case 'add-delay':
    case 'jitter':
    case 'filtered-on':
      return { tag: 'wrapper', inner: initialState(policy.inner) };
    case 'and-then':
      return {
        tag: 'and-then',
        left: initialState(policy.left),
        right: initialState(policy.right),
        onRight: false,
      };
    case 'policy-union':
    case 'policy-intersect':
      return { tag: 'pair', left: initialState(policy.left), right: initialState(policy.right) };
  }
}

function evaluate(
  policy: CompiledPolicy,
  state: PolicyState,
  elapsed: bigint,
  properties: Properties,
): { state: PolicyState; verdict: Verdict } {
  switch (policy.tag) {
    case 'periodic': {
      const count = counter(state);
      return {
        state: { tag: 'counter', count: increment(count) },
        verdict: retryAfter(policy.delay),
      };
    }
    case 'exponential': {
      const count = counter(state);
      return {
        state: { tag: 'counter', count: increment(count) },
        verdict: retryAfter(scaleDuration(policy.baseDelay, policy.factor ** count)),
      };
    }
    case 'fibonacci': {
      const count = counter(state);
      return {
        state: { tag: 'counter', count: increment(count) },
        verdict: retryAfter(fibonacci(policy.first, policy.second, increment(count))),
      };
    }
    case 'immediate': {
      const count = counter(state);
      return { state: { tag: 'counter', count: increment(count) }, verdict: retryAfter(0n) };
    }
    case 'never':
      requireState(state, 'terminal');
      return { state, verdict: giveUp() };
    case 'count-box': {
      requireState(state, 'count-box');
      if (state.attempts >= policy.maxRetries) return { state, verdict: giveUp() };
      const result = evaluate(policy.inner, state.inner, elapsed, properties);
      return {
        state: { tag: 'count-box', attempts: increment(state.attempts), inner: result.state },
        verdict: result.verdict,
      };
    }
    case 'time-box': {
      requireState(state, 'wrapper');
      if (elapsed >= policy.limit) return { state, verdict: giveUp() };
      return wrap(evaluate(policy.inner, state.inner, elapsed, properties));
    }
    case 'clamp-delay': {
      requireState(state, 'wrapper');
      const result = evaluate(policy.inner, state.inner, elapsed, properties);
      return transformDelay(result, (delay) => max(policy.minDelay, min(delay, policy.maxDelay)));
    }
    case 'add-delay': {
      requireState(state, 'wrapper');
      const result = evaluate(policy.inner, state.inner, elapsed, properties);
      return transformDelay(result, (delay) => saturatingAdd(delay, policy.delay));
    }
    case 'jitter': {
      requireState(state, 'wrapper');
      const result = evaluate(policy.inner, state.inner, elapsed, properties);
      return transformDelay(result, (delay) =>
        policy.factor === 0
          ? delay
          : saturatingAdd(delay, scaleDuration(delay, Math.random() * policy.factor)),
      );
    }
    case 'filtered-on': {
      requireState(state, 'wrapper');
      if (!matches(policy.predicate, properties)) return { state, verdict: giveUp() };
      return wrap(evaluate(policy.inner, state.inner, elapsed, properties));
    }
    case 'and-then': {
      requireState(state, 'and-then');
      if (state.onRight) {
        const right = evaluate(policy.right, state.right, elapsed, properties);
        return { state: { ...state, right: right.state }, verdict: right.verdict };
      }
      const left = evaluate(policy.left, state.left, elapsed, properties);
      if (left.verdict.tag === 'retry') {
        return { state: { ...state, left: left.state }, verdict: left.verdict };
      }
      const right = evaluate(policy.right, state.right, elapsed, properties);
      return {
        state: { tag: 'and-then', left: left.state, right: right.state, onRight: true },
        verdict: right.verdict,
      };
    }
    case 'policy-union':
    case 'policy-intersect': {
      requireState(state, 'pair');
      const left = evaluate(policy.left, state.left, elapsed, properties);
      const right = evaluate(policy.right, state.right, elapsed, properties);
      const verdict = combine(policy.tag, left.verdict, right.verdict);
      return { state: { tag: 'pair', left: left.state, right: right.state }, verdict };
    }
  }
}

function matches(predicate: CompiledPredicate, properties: Properties): boolean {
  switch (predicate.tag) {
    case 'pred-true':
      return true;
    case 'pred-false':
      return false;
    case 'prop-exists':
      return properties.has(predicate.property);
    case 'pred-and':
      return matches(predicate.left, properties) && matches(predicate.right, properties);
    case 'pred-or':
      return matches(predicate.left, properties) || matches(predicate.right, properties);
    case 'pred-not':
      return !matches(predicate.inner, properties);
    case 'prop-eq':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) === 0
      );
    case 'prop-neq':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) !== 0
      );
    case 'prop-gt':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) > 0
      );
    case 'prop-gte':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) >= 0
      );
    case 'prop-lt':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) < 0
      );
    case 'prop-lte':
      return (
        compare(
          requiredProperty(properties, predicate.property),
          predicate.value,
          predicate.property,
        ) <= 0
      );
    case 'prop-in': {
      const actual = requiredProperty(properties, predicate.property);
      let firstError: unknown;
      for (const value of predicate.values) {
        try {
          if (compare(actual, value, predicate.property) === 0) return true;
        } catch (error) {
          firstError ??= error;
        }
      }
      if (firstError !== undefined) throw firstError;
      return false;
    }
    case 'prop-matches':
      return globMatches(
        predicate.pattern,
        asText(requiredProperty(properties, predicate.property), predicate.property),
      );
    case 'prop-starts-with':
      return asText(
        requiredProperty(properties, predicate.property),
        predicate.property,
      ).startsWith(predicate.pattern);
    case 'prop-contains':
      return asText(requiredProperty(properties, predicate.property), predicate.property).includes(
        predicate.pattern,
      );
  }
}

function compare(actual: PredicateValue, expected: PredicateValue, property: string): number {
  if (actual.tag === expected.tag) {
    if (actual.val === expected.val) return 0;
    if (actual.tag === 'text' && expected.tag === 'text') {
      return compareText(actual.val, expected.val);
    }
    return actual.val < expected.val ? -1 : 1;
  }
  if (actual.tag === 'text' && expected.tag === 'integer') {
    if (!isSignedDecimalInteger(actual.val)) throw coercionError(property, actual, 'integer');
    const parsed = BigInt(actual.val);
    if (parsed < -(1n << 63n) || parsed > (1n << 63n) - 1n) {
      throw coercionError(property, actual, 'integer');
    }
    return parsed === expected.val ? 0 : parsed < expected.val ? -1 : 1;
  }
  if (actual.tag === 'integer' && expected.tag === 'text') {
    const text = actual.val.toString();
    return compareText(text, expected.val);
  }
  throw coercionError(property, actual, expected.tag);
}

function compareText(left: string, right: string): number {
  const encoder = new TextEncoder();
  const leftBytes = encoder.encode(left);
  const rightBytes = encoder.encode(right);
  const length = Math.min(leftBytes.length, rightBytes.length);
  for (let index = 0; index < length; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) {
      return leftBytes[index] < rightBytes[index] ? -1 : 1;
    }
  }
  return leftBytes.length === rightBytes.length ? 0 : leftBytes.length < rightBytes.length ? -1 : 1;
}

function isSignedDecimalInteger(value: string): boolean {
  if (value.length === 0) return false;
  let index = value[0] === '+' || value[0] === '-' ? 1 : 0;
  if (index === value.length) return false;
  for (; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) return false;
  }
  return true;
}

function globMatches(pattern: string, value: string): boolean {
  let source = '^';
  for (const character of pattern) {
    if (character === '*') source += '.*';
    else if (character === '?') source += '.';
    else source += character.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`${source}$`).test(value);
}

function projectProperties(input?: RetryProperties): Properties {
  const result = new Map<string, PredicateValue>();
  if (input === undefined) return result;
  const entries = Symbol.iterator in Object(input) ? input : Object.entries(input);
  for (const entry of entries as Iterable<readonly [string, PredicateValueInput]>) {
    if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== 'string') {
      throw invalid('retry properties must contain [string, value] entries');
    }
    result.set(entry[0], toRawPredicateValue(entry[1], `retry property '${entry[0]}'`));
  }
  return result;
}

function combine(
  kind: 'policy-union' | 'policy-intersect',
  left: Verdict,
  right: Verdict,
): Verdict {
  if (kind === 'policy-union') {
    if (left.tag === 'give-up') return right;
    if (right.tag === 'give-up') return left;
    return retryAfter(min(left.delay, right.delay));
  }
  if (left.tag === 'give-up' || right.tag === 'give-up') return giveUp();
  return retryAfter(max(left.delay, right.delay));
}

function transformDelay(
  result: { state: PolicyState; verdict: Verdict },
  transform: (delay: bigint) => bigint,
): { state: PolicyState; verdict: Verdict } {
  return {
    state: { tag: 'wrapper', inner: result.state },
    verdict:
      result.verdict.tag === 'retry' ? retryAfter(transform(result.verdict.delay)) : result.verdict,
  };
}

function wrap(result: { state: PolicyState; verdict: Verdict }) {
  return { state: { tag: 'wrapper', inner: result.state } as PolicyState, verdict: result.verdict };
}

function fibonacci(first: bigint, second: bigint, nth: number): bigint {
  if (nth === 1) return first;
  if (nth === 2) return second;
  let left = first;
  let right = second;
  for (let current = 3; current <= nth; current += 1) {
    [left, right] = [right, saturatingAdd(left, right)];
  }
  return right;
}

function scaleDuration(duration: bigint, factor: number): bigint {
  if (!Number.isFinite(factor) || factor <= 0) return 0n;
  const seconds = Number(duration / 1_000_000_000n) + Number(duration % 1_000_000_000n) / 1e9;
  const value = seconds * factor;
  if (!Number.isFinite(value) || value >= 2 ** 64) return DURATION_MAX;
  return floatSecondsToDuration(value);
}

function floatSecondsToDuration(seconds: number): bigint {
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, seconds);
  const bits = view.getBigUint64(0);
  const exponentBits = Number((bits >> 52n) & 0x7ffn);
  const fraction = bits & ((1n << 52n) - 1n);
  const mantissa = exponentBits === 0 ? fraction : fraction | (1n << 52n);
  const exponent = (exponentBits === 0 ? -1022 : exponentBits - 1023) - 52;
  const scaled = mantissa * 1_000_000_000n;
  if (exponent >= 0) return min(scaled << BigInt(exponent), DURATION_MAX);

  const divisor = 1n << BigInt(-exponent);
  const wholeNanoseconds = scaled / divisor;
  const remainder = scaled % divisor;
  const roundUp =
    remainder * 2n > divisor || (remainder * 2n === divisor && wholeNanoseconds % 2n === 1n);
  return min(wholeNanoseconds + (roundUp ? 1n : 0n), DURATION_MAX);
}

function saturatingAdd(left: bigint, right: bigint): bigint {
  return min(left + right, DURATION_MAX);
}

function elapsedNanoseconds(startedAt: number): bigint {
  return BigInt(Math.max(0, Math.floor((monotonicMilliseconds() - startedAt) * 1_000_000)));
}

function monotonicMilliseconds(): number {
  return typeof globalThis.performance === 'undefined' ? Date.now() : globalThis.performance.now();
}

async function sleep(nanoseconds: bigint, signal?: AbortSignal): Promise<void> {
  let milliseconds = Number((nanoseconds + 999_999n) / 1_000_000n);
  do {
    throwIfAborted(signal);
    const chunk = Math.min(milliseconds, MAX_TIMER_DELAY_MS);
    let timer: ReturnType<typeof globalThis.setTimeout>;
    const delay = new Promise<void>((resolve) => {
      timer = globalThis.setTimeout(resolve, chunk);
    });
    await awaitAbortable(delay, signal, () => globalThis.clearTimeout(timer));
    milliseconds -= chunk;
  } while (milliseconds > 0);
  throwIfAborted(signal);
}

function requiredProperty(properties: Properties, property: string): PredicateValue {
  const value = properties.get(property);
  if (value === undefined)
    throw new RetryPolicyError(`retry property '${property}' was not provided`);
  return value;
}

function asText(value: PredicateValue, property: string): string {
  if (value.tag === 'text') return value.val;
  if (value.tag === 'integer') return value.val.toString();
  throw coercionError(property, value, 'text');
}

function coercionError(property: string, actual: PredicateValue, target: string): RetryPolicyError {
  return new RetryPolicyError(
    `retry property '${property}' with ${actual.tag} value cannot be compared as ${target}`,
  );
}

function counter(state: PolicyState): number {
  requireState(state, 'counter');
  return state.count;
}

function requireState<T extends PolicyState['tag']>(
  state: PolicyState,
  tag: T,
): asserts state is Extract<PolicyState, { tag: T }> {
  if (state.tag !== tag)
    throw invalid(`internal state '${state.tag}' does not match '${tag}' policy`);
}

function increment(value: number): number {
  return Math.min(value + 1, UINT32_MAX);
}

function retryAfter(delay: bigint): Verdict {
  return { tag: 'retry', delay };
}

function giveUp(): Verdict {
  return { tag: 'give-up' };
}

function min(left: bigint, right: bigint): bigint {
  return left < right ? left : right;
}

function max(left: bigint, right: bigint): bigint {
  return left > right ? left : right;
}

function indexedNode<T>(nodes: T[], index: number, kind: string): T {
  if (index < 0 || index >= nodes.length || !isObject(nodes[index])) {
    throw invalid(`${kind} node index ${index} is out of bounds`);
  }
  return nodes[index];
}

function validIndex(value: unknown, context: string): number {
  if (
    !Number.isInteger(value) ||
    (value as number) < -0x8000_0000 ||
    (value as number) > 0x7fff_ffff
  ) {
    throw invalid(`${context} must be a signed 32-bit integer`);
  }
  return value as number;
}

function validPair(value: unknown, context: string): [unknown, unknown] {
  if (!Array.isArray(value) || value.length !== 2) throw invalid(`${context} must be a pair`);
  return [value[0], value[1]];
}

function validDuration(value: unknown, context: string): bigint {
  if (typeof value !== 'bigint' || value < 0n || value > UINT64_MAX) {
    throw invalid(`${context} must be an unsigned 64-bit nanosecond duration`);
  }
  return value;
}

function validUint32(value: unknown, context: string): number {
  if (!Number.isInteger(value) || (value as number) < 0 || (value as number) > UINT32_MAX) {
    throw invalid(`${context} must be an unsigned 32-bit integer`);
  }
  return value as number;
}

function validPositiveFactor(value: unknown, context: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    throw invalid(`${context} must be a finite number greater than 0`);
  }
  return value;
}

function validNonNegativeFactor(value: unknown, context: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw invalid(`${context} must be a finite non-negative number`);
  }
  return value;
}

function validPredicateValue(value: unknown): PredicateValue {
  if (!isObject(value)) throw invalid('predicate value must be a tagged value');
  if (value.tag === 'text' && typeof value.val === 'string') {
    return { tag: 'text', val: value.val };
  }
  if (value.tag === 'boolean' && typeof value.val === 'boolean') {
    return { tag: 'boolean', val: value.val };
  }
  if (
    value.tag === 'integer' &&
    typeof value.val === 'bigint' &&
    value.val >= -(1n << 63n) &&
    value.val <= (1n << 63n) - 1n
  ) {
    return { tag: 'integer', val: value.val };
  }
  throw invalid('predicate value does not match its WIT tag');
}

function validObject(value: unknown, context: string): Record<string, unknown> {
  if (!isObject(value)) throw invalid(`${context} must be an object`);
  return value;
}

function validString(value: unknown, context: string): string {
  if (typeof value !== 'string') throw invalid(`${context} must be a string`);
  return value;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function invalid(message: string): RetryPolicyError {
  return new RetryPolicyError(`Invalid retry policy: ${message}`);
}
