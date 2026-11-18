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

export {
  type NamedRetryPolicy,
  type PredicateValue,
  type RetryPolicy,
  type RetryPredicate,
  getRetryPolicies,
  getRetryPolicyByName,
  resolveRetryPolicy,
  removeRetryPolicy,
} from 'golem:api/retry@1.5.0';

import { setRetryPolicy as rawSetRetryPolicy } from 'golem:api/retry@1.5.0';
import { RetryPolicyScope } from 'golem:api/retry-scope@1.5.0';
import { executeWithDrop } from './guard';
import {
  Duration,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  type DurationInput,
  type NamedPolicyInput,
  type PolicyInput,
  type PredicateValueInput,
  toRawDuration,
  toRawNamedPolicy,
  toRawPolicy,
  toRawPredicate,
  toRawPredicateValue,
} from './retryBuilder';

export {
  Duration,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  type DurationInput,
  type NamedPolicyInput,
  type PolicyInput,
  type PredicateValueInput,
  toRawDuration,
  toRawNamedPolicy,
  toRawPolicy,
  toRawPredicate,
  toRawPredicateValue,
};

export { retry, RetryPolicyError, type RetryOptions, type RetryProperties } from './retryExecutor';

export function setRetryPolicy(policy: NamedPolicyInput): void {
  rawSetRetryPolicy(toRawNamedPolicy(policy));
}

/**
 * RetryPolicyGuard holds an open retry policy scope. While the scope is open, its named policy
 * overrides the agent-wide policy with the same name for the host calls of the current task.
 * The scope changes no agent-wide policy and records no oplog entry.
 *
 * JavaScript code of one agent runs on one task, so other async work of the same invocation
 * that runs while the scope is open also sees the policy.
 *
 * You must call drop on the guard once you are finished using it.
 */
export class RetryPolicyGuard {
  constructor(private scope: RetryPolicyScope | undefined) {}
  drop() {
    const scope = this.scope;
    this.scope = undefined;
    (scope as unknown as { [Symbol.dispose](): void } | undefined)?.[Symbol.dispose]();
  }
}

/**
 * Opens a retry policy scope and returns its guard. The policy applies until the guard is
 * dropped.
 * @param policy - The named retry policy to apply, either as a raw WIT shape or a high-level NamedPolicy.
 * @returns A RetryPolicyGuard instance.
 */
export function useRetryPolicy(policy: NamedPolicyInput): RetryPolicyGuard {
  return new RetryPolicyGuard(new RetryPolicyScope(toRawNamedPolicy(policy)));
}

export function withRetryPolicy<R>(policy: NamedPolicyInput, f: () => Promise<R>): Promise<R>;
export function withRetryPolicy<R>(policy: NamedPolicyInput, f: () => R): R;

/**
 * Executes a function inside a retry policy scope for a named retry policy.
 * Supports both sync and async callbacks.
 *
 * The scope ends when the function returns or its Promise settles, also when it fails.
 *
 * Use {@link retry} to retry guest code. Host HTTP retry remains host-managed.
 *
 * @param policy - The named retry policy to set.
 * @param f - The function to execute (sync or async).
 * @returns The result of the executed function, or a Promise if an async function was passed.
 */
export function withRetryPolicy<R>(
  policy: NamedPolicyInput,
  f: () => R | Promise<R>,
): R | Promise<R> {
  const guard = useRetryPolicy(policy);
  return executeWithDrop([guard], f);
}
