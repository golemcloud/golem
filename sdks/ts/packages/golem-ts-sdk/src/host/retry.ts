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
  setRetryPolicy,
  removeRetryPolicy,
} from 'golem:api/retry@1.5.0';

import {
  type NamedRetryPolicy,
  getRetryPolicyByName,
  setRetryPolicy,
  removeRetryPolicy,
} from 'golem:api/retry@1.5.0';
import { executeWithDrop } from './guard';

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
      setRetryPolicy(this.previous);
    } else {
      removeRetryPolicy(this.name);
    }
  }
}

/**
 * Temporarily sets a named retry policy and returns a guard.
 * When the guard is dropped, the previous policy with the same name is restored
 * (or removed if it didn't exist).
 * @param policy - The named retry policy to set.
 * @returns A RetryPolicyGuard instance.
 */
export function useRetryPolicy(policy: NamedRetryPolicy): RetryPolicyGuard {
  const previous = getRetryPolicyByName(policy.name);
  const name = policy.name;
  setRetryPolicy(policy);
  return new RetryPolicyGuard(previous, name);
}

/**
 * Executes a function with a named retry policy temporarily set.
 * The policy is restored (or removed) after the function completes.
 * @param policy - The named retry policy to set.
 * @param f - The function to execute.
 * @returns The result of the executed function.
 */
export function withRetryPolicy<R>(policy: NamedRetryPolicy, f: () => R): R {
  const guard = useRetryPolicy(policy);
  return executeWithDrop([guard], f);
}
