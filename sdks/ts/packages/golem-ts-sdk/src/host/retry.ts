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

import {
  type NamedRetryPolicy,
  getRetryPolicyByName,
  setRetryPolicy as rawSetRetryPolicy,
  removeRetryPolicy as rawRemoveRetryPolicy,
} from 'golem:api/retry@1.5.0';
import { executeWithDrop } from './guard';
import {
  Duration,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  type DurationInput,
  type NamedPolicyInput,
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
  type PredicateValueInput,
  toRawDuration,
  toRawNamedPolicy,
  toRawPolicy,
  toRawPredicate,
  toRawPredicateValue,
};

export function setRetryPolicy(policy: NamedPolicyInput): void {
  rawSetRetryPolicy(toRawNamedPolicy(policy));
}

/**
 * RetryPolicyGuard is a guard type that restores the previous retry policy on drop.
 * If the policy existed before, it is restored; if it was newly added, it is removed.
 * You must call drop on the guard once you are finished using it.
 */
export class RetryPolicyGuard {
  constructor(
    private previous: NamedRetryPolicy | undefined,
    private name: string,
  ) {}
  drop() {
    if (this.previous !== undefined) {
      rawSetRetryPolicy(this.previous);
    } else {
      rawRemoveRetryPolicy(this.name);
    }
  }
}

/**
 * Temporarily sets a named retry policy and returns a guard.
 * When the guard is dropped, the previous policy with the same name is restored
 * (or removed if it didn't exist).
 * @param policy - The named retry policy to set, either as a raw WIT shape or a high-level NamedPolicy.
 * @returns A RetryPolicyGuard instance.
 */
export function useRetryPolicy(policy: NamedPolicyInput): RetryPolicyGuard {
  const rawPolicy = toRawNamedPolicy(policy);
  const previous = getRetryPolicyByName(rawPolicy.name);
  const name = rawPolicy.name;
  rawSetRetryPolicy(rawPolicy);
  return new RetryPolicyGuard(previous, name);
}

/**
 * Executes a function with a named retry policy temporarily set.
 * Supports both sync and async callbacks.
 * The policy is restored (or removed) after the function completes.
 * @param policy - The named retry policy to set, either as a raw WIT shape or a high-level NamedPolicy.
 * @param f - The function to execute (sync or async).
 * @returns The result of the executed function, or a Promise if an async function was passed.
 */
export function withRetryPolicy<R>(policy: NamedPolicyInput, f: () => R): R {
  const guard = useRetryPolicy(policy);
  return executeWithDrop([guard], f);
}
