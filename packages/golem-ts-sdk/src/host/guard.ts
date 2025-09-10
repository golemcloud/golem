// Copyright 2024-2025 Golem Cloud
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

import {
  type OplogIndex,
  type PersistenceLevel,
  type RetryPolicy,
  getIdempotenceMode,
  getOplogPersistenceLevel,
  getRetryPolicy,
  markBeginOperation,
  markEndOperation,
  setIdempotenceMode,
  setOplogPersistenceLevel,
  setRetryPolicy,
} from './hostapi';

/**
 * PersistenceLevelGuard is a guard type that sets the persistence level for the oplog.
 * You must call drop on the guard once you are finished using it.
 */
export class PersistenceLevelGuard {
  constructor(private originalLevel: PersistenceLevel) {}
  drop() {
    setOplogPersistenceLevel(this.originalLevel);
  }
}

/**
 * Sets the persistence level for the oplog and returns a guard.
 * You must call drop on the guard once you are finished using it.
 * @param level - The persistence level to set.
 * @returns A PersistenceLevelGuard instance.
 */
export function usePersistenceLevel(level: PersistenceLevel) {
  const originalLevel = getOplogPersistenceLevel();
  setOplogPersistenceLevel(level);
  return new PersistenceLevelGuard(originalLevel);
}

/**
 * Executes a function with a specific persistence level for the oplog.
 * @param level - The persistence level to set.
 * @param f - The function to execute.
 * @returns The result of the executed function.
 */
export function withPersistenceLevel<R>(
  level: PersistenceLevel,
  f: () => R,
): R {
  const guard = usePersistenceLevel(level);
  return executeWithDrop([guard], f);
}

/**
 * IdempotenceModeGuard is a guard type that sets the idempotence mode.
 * You must call drop on the guard once you are finished using it.
 */
export class IdempotenceModeGuard {
  constructor(private original: boolean) {}
  drop() {
    setIdempotenceMode(this.original);
  }
}

/**
 * Sets the idempotence mode and returns a guard.
 * You must call drop on the guard once you are finished using it.
 * @param mode - The idempotence mode to set.
 * @returns An IdempotenceModeGuard instance.
 */
export function useIdempotenceMode(mode: boolean): IdempotenceModeGuard {
  const original = getIdempotenceMode();
  setIdempotenceMode(mode);
  return new IdempotenceModeGuard(original);
}

/**
 * Executes a function with a specific idempotence mode.
 * @param mode - The idempotence mode to set.
 * @param f - The function to execute.
 * @returns The result of the executed function.
 */
export function withIdempotenceMode<R>(mode: boolean, f: () => R): R {
  const guard = useIdempotenceMode(mode);
  return executeWithDrop([guard], f);
}

/**
 * RetryPolicyGuard is a guard type that sets the retry policy.
 * You must call drop on the guard once you are finished using it.
 */
export class RetryPolicyGuard {
  constructor(private original: RetryPolicy) {}
  drop() {
    setRetryPolicy(this.original);
  }
}

/**
 * Sets the retry policy and returns a guard.
 * You must call drop on the guard once you are finished using it.
 * @param policy - The retry policy to set.
 * @returns A RetryPolicyGuard instance.
 */
export function useRetryPolicy(policy: RetryPolicy): RetryPolicyGuard {
  const original = getRetryPolicy();
  setRetryPolicy(policy);
  return new RetryPolicyGuard(original);
}

/**
 * Executes a function with a specific retry policy.
 * @param policy - The retry policy to set.
 * @param f - The function to execute.
 * @returns The result of the executed function.
 */
export function withRetryPolicy<R>(policy: RetryPolicy, f: () => R): R {
  const guard = useRetryPolicy(policy);
  return executeWithDrop([guard], f);
}

/**
 * AtomicOperationGuard is a guard type that marks the beginning and end of an atomic operation.
 * You must call drop on the guard once you are finished using it.
 */
export class AtomicOperationGuard {
  constructor(private begin: OplogIndex) {}
  drop() {
    markEndOperation(this.begin);
  }
}

/**
 * Marks the beginning of an atomic operation and returns a guard.
 * You must call drop on the guard once you are finished using it.
 * @returns An AtomicOperationGuard instance.
 */
export function markAtomicOperation(): AtomicOperationGuard {
  const begin = markBeginOperation();
  return new AtomicOperationGuard(begin);
}

/**
 * Executes a function atomically.
 * @param f - The function to execute atomically.
 * @returns The result of the executed function.
 */
export function atomically<T>(f: () => T): T {
  const guard = markAtomicOperation();
  return executeWithDrop([guard], f);
}

/**
 * Executes a function and automatically drops the provided resources after execution.
 * @param resources - An array of resources to be dropped after execution.
 * @param fn - The function to execute.
 * @returns The result of the executed function.
 */
export function executeWithDrop<Resource extends { drop: () => void }, R>(
  resources: [Resource],
  fn: () => R,
): R {
  try {
    const result = fn();
    dropAll(true, resources);
    return result;
  } catch (e) {
    dropAll(false, resources);
    throw e;
  }
}

/**
 * Drops all the provided resources and collects any errors that occur during the process.
 * @param resources - An array of resources to be dropped.
 * @throws DropError if any errors occur during the dropping process.
 */
export function dropAll<Resource extends { drop: () => void }>(
  throwOnError: boolean,
  resources: [Resource],
) {
  const errors = [];
  for (const resource of resources) {
    try {
      resource.drop();
    } catch (e) {
      if (e instanceof Error) {
        errors.push(e);
      }
    }
  }
  if (throwOnError && errors.length > 0) {
    throw new DropError(errors);
  }
}

/**
 * Custom error class for errors that occur during the dropping of resources.
 */
class DropError extends Error {
  constructor(public errors: Error[]) {
    const message = errors.map((e) => e.message).join(', ');
    super(`Error dropping resources: ${message}`);
  }
}
