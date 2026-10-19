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

export function isPromiseLike(value: unknown): value is Promise<unknown> {
  return value !== null && value !== undefined && typeof (value as any).then === 'function';
}

export function executeWithDrop<Resource extends { drop: () => void }, R>(
  resources: [Resource],
  fn: () => Promise<R>,
): Promise<R>;
export function executeWithDrop<Resource extends { drop: () => void }, R>(
  resources: [Resource],
  fn: () => R,
): R;

/**
 * Executes a function and automatically drops the provided resources after execution.
 * Supports both sync and async callbacks — if the callback returns a Promise,
 * the resources are dropped after the Promise settles.
 * @param resources - An array of resources to be dropped after execution.
 * @param fn - The function to execute (sync or async).
 * @returns The result of the executed function, or a Promise if an async function was passed.
 */
export function executeWithDrop<Resource extends { drop: () => void }, R>(
  resources: [Resource],
  fn: () => R | Promise<R>,
): R | Promise<R> {
  try {
    const result = fn();
    if (isPromiseLike(result)) {
      return result.then(
        (val) => {
          dropAll(true, resources);
          return val;
        },
        (err) => {
          dropAll(false, resources);
          throw err;
        },
      ) as R;
    }
    dropAll(true, resources);
    return result;
  } catch (e) {
    dropAll(false, resources);
    throw e;
  }
}

/**
 * Executes an async function and automatically drops the provided resources after execution.
 * @param resources - An array of resources to be dropped after execution.
 * @param fn - The async function to execute.
 * @returns A promise resolving to the result of the executed function.
 * @deprecated Use {@link executeWithDrop} instead, which handles both sync and async callbacks.
 */
export async function executeWithDropAsync<
  Resource extends {
    drop: () => void;
  },
  R,
>(resources: [Resource], fn: () => Promise<R>): Promise<R> {
  try {
    const result = await fn();
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
export function dropAll<
  Resource extends {
    drop: () => void;
  },
>(throwOnError: boolean, resources: [Resource]) {
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
