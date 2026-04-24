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

import { type OplogIndex, getOplogIndex, setOplogIndex } from './hostapi';
import { isPromiseLike } from './guard';
import { Result } from './result';

/**
 * A checkpoint that captures the current oplog index and can revert execution to that point.
 *
 * Use {@link checkpoint} to create a new checkpoint, or {@link withCheckpoint} /
 * {@link withCheckpointTry} to execute a block with automatic revert on failure.
 */
export class Checkpoint {
  private readonly oplogIndex: OplogIndex;

  constructor() {
    this.oplogIndex = getOplogIndex();
  }

  /**
   * Reverts execution to the oplog index captured when this checkpoint was created.
   * This function never returns.
   */
  revert(): never {
    setOplogIndex(this.oplogIndex);
    throw new Error('Unreachable: reverted to checkpoint');
  }

  /**
   * Returns the successful value of the result, or reverts to the checkpoint if the result is an error.
   * @param result - The result to unwrap.
   * @returns The successful value.
   */
  unwrapOrRevert<T, E>(result: Result<T, E>): T {
    if (result.isOk()) return result.val;
    this.revert();
  }

  /**
   * Runs the given function that returns a Result, reverting to the checkpoint if the result is an error.
   * Supports both sync and async callbacks.
   * @param fn - The function to execute (sync or async).
   * @returns The successful value, or a Promise of it if an async function was passed.
   */
  runOrRevert<T, E>(fn: () => Result<T, E>): T;
  runOrRevert<T, E>(fn: () => Promise<Result<T, E>>): Promise<T>;
  runOrRevert<T, E>(fn: () => Result<T, E> | Promise<Result<T, E>>): T | Promise<T> {
    const result = fn();
    if (isPromiseLike(result)) {
      return (result as Promise<Result<T, E>>).then((r) => r.unwrapOrRevert(this));
    }
    return (result as Result<T, E>).unwrapOrRevert(this);
  }

  /**
   * Executes the given function and returns its result. If the function throws
   * (or the returned Promise rejects), reverts to the checkpoint.
   * Supports both sync and async callbacks.
   * @param fn - The function to execute (sync or async).
   * @returns The result of the function, or a Promise of it if an async function was passed.
   */
  tryOrRevert<T>(fn: () => T): T {
    try {
      const result = fn();
      if (isPromiseLike(result)) {
        return (result as Promise<unknown>).catch(() => this.revert()) as T;
      }
      return result;
    } catch {
      this.revert();
    }
  }

  /**
   * Asserts a condition. If the condition is false, reverts to the checkpoint.
   * @param condition - The condition to assert.
   */
  assertOrRevert(condition: boolean): void {
    if (!condition) {
      this.revert();
    }
  }
}

/**
 * Creates a new checkpoint at the current oplog index.
 * @returns A new {@link Checkpoint} instance.
 */
export function checkpoint(): Checkpoint {
  return new Checkpoint();
}

/**
 * Creates a checkpoint and executes the given function. If the function returns an error Result,
 * reverts to the checkpoint. Supports both sync and async callbacks.
 * @param fn - The function to execute with a checkpoint. Must return a Result (or Promise of Result).
 * @returns The successful value, or a Promise of it if an async function was passed.
 */
export function withCheckpoint<T, E>(fn: (cp: Checkpoint) => Result<T, E>): T;
export function withCheckpoint<T, E>(fn: (cp: Checkpoint) => Promise<Result<T, E>>): Promise<T>;
export function withCheckpoint<T, E>(
  fn: (cp: Checkpoint) => Result<T, E> | Promise<Result<T, E>>,
): T | Promise<T> {
  const cp = new Checkpoint();
  const result = fn(cp);
  if (isPromiseLike(result)) {
    return (result as Promise<Result<T, E>>).then((r) => r.unwrapOrRevert(cp));
  }
  return (result as Result<T, E>).unwrapOrRevert(cp);
}

/**
 * Creates a checkpoint and executes the given function. If the function throws
 * (or the returned Promise rejects), reverts to the checkpoint.
 * Supports both sync and async callbacks.
 * @param fn - The function to execute with a checkpoint (sync or async).
 * @returns The result of the function, or a Promise of it if an async function was passed.
 */
export function withCheckpointTry<T>(fn: (cp: Checkpoint) => T): T {
  const cp = new Checkpoint();
  try {
    const result = fn(cp);
    if (isPromiseLike(result)) {
      return (result as Promise<unknown>).catch(() => cp.revert()) as T;
    }
    return result;
  } catch {
    return cp.revert();
  }
}

/**
 * @deprecated Use {@link withCheckpoint} instead, which handles both sync and async callbacks.
 */
export async function withCheckpointAsync<T, E>(
  fn: (cp: Checkpoint) => Promise<Result<T, E>>,
): Promise<T> {
  return withCheckpoint(fn);
}

/**
 * @deprecated Use {@link withCheckpointTry} instead, which handles both sync and async callbacks.
 */
export async function withCheckpointTryAsync<T>(fn: (cp: Checkpoint) => Promise<T>): Promise<T> {
  return withCheckpointTry(fn);
}
