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
import { Result } from './result';

/**
 * A checkpoint that captures the current oplog index and can revert execution to that point.
 *
 * Use {@link checkpoint} to create a new checkpoint, or {@link withCheckpoint} /
 * {@link withCheckpointAsync} to execute a block with automatic revert on failure.
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
   * @param fn - The function to execute.
   * @returns The successful value.
   */
  runOrRevert<T, E>(fn: () => Result<T, E>): T {
    return fn().unwrapOrRevert(this);
  }

  /**
   * Executes the given function and returns its result. If the function throws, reverts to the checkpoint.
   * @param fn - The function to execute.
   * @returns The result of the function.
   */
  tryOrRevert<T>(fn: () => T): T {
    try {
      return fn();
    } catch {
      this.revert();
    }
  }

  /**
   * Executes the given async function and returns its result. If the promise rejects, reverts to the checkpoint.
   * @param fn - The async function to execute.
   * @returns A promise resolving to the result of the function.
   */
  async tryOrRevertAsync<T>(fn: () => Promise<T>): Promise<T> {
    try {
      return await fn();
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
 * reverts to the checkpoint.
 * @param fn - The function to execute with a checkpoint. Must return a Result.
 * @returns The successful value.
 */
export function withCheckpoint<T, E>(fn: (cp: Checkpoint) => Result<T, E>): T {
  const cp = new Checkpoint();
  return fn(cp).unwrapOrRevert(cp);
}

/**
 * Creates a checkpoint and executes the given async function. If the returned promise resolves
 * to an error Result, reverts to the checkpoint.
 * @param fn - The async function to execute with a checkpoint. Must return a Promise of Result.
 * @returns A promise resolving to the successful value.
 */
export async function withCheckpointAsync<T, E>(
  fn: (cp: Checkpoint) => Promise<Result<T, E>>,
): Promise<T> {
  const cp = new Checkpoint();
  return (await fn(cp)).unwrapOrRevert(cp);
}

/**
 * Creates a checkpoint and executes the given function. If the function throws, reverts to the checkpoint.
 * @param fn - The function to execute with a checkpoint.
 * @returns The result of the function.
 */
export function withCheckpointTry<T>(fn: (cp: Checkpoint) => T): T {
  const cp = new Checkpoint();
  try {
    return fn(cp);
  } catch {
    return cp.revert();
  }
}

/**
 * Creates a checkpoint and executes the given async function. If the promise rejects, reverts to the checkpoint.
 * @param fn - The async function to execute with a checkpoint.
 * @returns A promise resolving to the result of the function.
 */
export async function withCheckpointTryAsync<T>(fn: (cp: Checkpoint) => Promise<T>): Promise<T> {
  const cp = new Checkpoint();
  try {
    return await fn(cp);
  } catch {
    return cp.revert();
  }
}
