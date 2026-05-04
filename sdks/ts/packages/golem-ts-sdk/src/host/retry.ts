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
import { isPromiseLike } from './guard';
import { trap } from './hostapi';
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

export function withRetryPolicy<R>(policy: NamedPolicyInput, f: () => Promise<R>): Promise<R>;
export function withRetryPolicy<R>(policy: NamedPolicyInput, f: () => R): R;

/**
 * Executes a function with a named retry policy temporarily set.
 * Supports both sync and async callbacks.
 *
 * On failure the worker is immediately terminated and retried by the executor
 * according to the active retry policy. This has two practical consequences:
 *
 * - Errors thrown inside `f` always trigger the retry policy — this is the
 *   recommended way to make user-land exceptions subject to executor-level
 *   retries.
 * - The callback cannot be wrapped in a `try/catch` to suppress the retry:
 *   any thrown error unconditionally causes the worker to be retried.
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
  try {
    const result = f();
    if (isPromiseLike(result)) {
      return result.then(
        (val) => {
          guard.drop();
          return val;
        },
        (err) => {
          // Leave the retry policy active and terminate the worker so the
          // executor retries the invocation with the policy applied.
          trap(`withRetryPolicy: ${formatErrorForTrap(err)}`);
          throw err;
        },
      ) as R;
    }
    guard.drop();
    return result;
  } catch (e) {
    trap(`withRetryPolicy: ${formatErrorForTrap(e)}`);
    throw e;
  }
}

function formatErrorForTrap(err: unknown): string {
  if (err instanceof Error) {
    return err.stack ?? `${err.name}: ${err.message}`;
  }
  try {
    return String(err);
  } catch {
    return '<unprintable error>';
  }
}
