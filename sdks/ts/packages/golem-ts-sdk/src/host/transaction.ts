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

import { executeWithDropAsync, markAtomicOperation } from './guard';
import { type OplogIndex, getOplogIndex, setOplogIndex } from './hostapi';
import { Result } from './result';

/**
 * Represents an atomic operation of the transaction which has a rollback action.
 *
 * Implement this interface and use it within a `transaction` block.
 * Operations can also be constructed from closures using `operation`.
 */
export interface Operation<In, Out, Err> {
  /**
   * The action to execute.
   * @param input - The input to the operation.
   * @returns A promise resolving to the result of the operation.
   */
  execute(input: In): Promise<Result<Out, Err>>;

  /**
   * Compensation to perform in case of failure.
   * Compensations should not throw errors.
   * @param input - The input to the operation.
   * @param result - The result of the operation.
   * @returns A promise resolving to the result of the compensation.
   */
  compensate(input: In, result: Out): Promise<Result<void, Err>>;
}

/**
 * Creates an Operation from the provided execute and compensate functions.
 * @param execute - The function to execute the operation.
 * @param compensate - The function to compensate the operation in case of failure.
 * @returns The created Operation.
 */
export function operation<In, Out, Err>(
  execute: (input: In) => Promise<Result<Out, Err>>,
  compensate: (input: In, result: Out) => Promise<Result<void, Err>>,
): Operation<In, Out, Err> {
  return new OperationImpl(execute, compensate);
}

class OperationImpl<In, Out, Err> implements Operation<In, Out, Err> {
  constructor(
    public readonly execute: (input: In) => Promise<Result<Out, Err>>,
    public readonly compensate: (input: In, result: Out) => Promise<Result<void, Err>>,
  ) {}
}

class InfallibleTransaction {
  private compensations: (() => Promise<void>)[] = [];

  constructor(private readonly beginOplogIndex: OplogIndex) {}

  /**
   * Executes an operation within the infallible transaction.
   * @param operation - The operation to execute.
   * @param input - The input to the operation.
   * @returns A promise resolving to the result of the operation.
   */
  async execute<In, Out, Err>(operation: Operation<In, Out, Err>, input: In): Promise<Out> {
    const result = await operation.execute(input);
    if (result.isOk()) {
      this.compensations.push(
        // Compensations cannot fail in infallible transactions.
        async () => {
          const compensationResult = await operation.compensate(input, result.val);
          if (compensationResult.isErr()) {
            throw new Error('Compensation action failed');
          }
        },
      );
      return result.val;
    } else {
      await this.retry();
      throw new Error('Unreachable code');
    }
  }

  private async retry(): Promise<void> {
    // Rollback all the compensations in reverse order
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      await this.compensations[i]();
    }
    setOplogIndex(this.beginOplogIndex);
  }
}

class FallibleTransaction<Err> {
  private compensations: (() => Promise<Result<void, Err>>)[] = [];

  /**
   * Executes an operation within the fallible transaction.
   * @param operation - The operation to execute.
   * @param input - The input to the operation.
   * @returns A promise resolving to the result of the operation.
   */
  async execute<In, Out, OpErr extends Err>(
    operation: Operation<In, Out, OpErr>,
    input: In,
  ): Promise<Result<Out, Err>> {
    const result = await operation.execute(input);
    if (result.isOk()) {
      this.compensations.push(async () => {
        return await operation.compensate(input, result.val);
      });
      return result;
    } else {
      return result;
    }
  }

  /**
   * Handles the failure of the fallible transaction.
   * @param error - The error that caused the failure.
   * @returns A promise resolving to the transaction failure result.
   */
  async onFailure(error: Err): Promise<TransactionFailure<Err>> {
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      const compensationResult = await this.compensations[i]();
      if (compensationResult.isErr()) {
        return {
          type: 'FailedAndRolledBackPartially',
          error,
          compensationFailure: compensationResult.val,
        };
      }
    }
    return {
      type: 'FailedAndRolledBackCompletely',
      error,
    };
  }
}

export type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

export type TransactionFailure<Err> =
  | {
      type: 'FailedAndRolledBackCompletely';
      error: Err;
    }
  | {
      type: 'FailedAndRolledBackPartially';
      error: Err;
      compensationFailure: Err;
    };

/**
 * Executes an infallible transaction.
 *
 * InfallibleTransaction is a sequence of operations that are executed in a way that if any of the
 * operations or the underlying Golem executor fails, the whole transaction is going to
 * be retried.
 *
 * In addition to that, **user level failures** (represented by the `Result::Err` value
 * of an operation) lead to performing the compensation actions of each already performed operation
 * in reverse order.
 *
 * Fatal errors (panic) and external executor failures currently cannot perform the
 * rollback actions.
 *
 * @param f - The async function that defines the transaction.
 * @returns A promise resolving to the result of the transaction.
 */
export async function infallibleTransaction<Out>(
  f: (tx: InfallibleTransaction) => Promise<Out>,
): Promise<Out> {
  const guard = markAtomicOperation();
  const beginOplogIndex = getOplogIndex();
  const tx = new InfallibleTransaction(beginOplogIndex);
  return executeWithDropAsync([guard], () => f(tx));
}

/**
 * Executes a fallible transaction.
 *
 * FallibleTransaction is a sequence of operations that are executed in a way that if any of the
 * operations fails, all the already performed operation's compensation actions get executed in
 * reverse order.
 *
 * In case of fatal errors (panic) and external executor failures, it does not perform the
 * compensation actions and the whole transaction gets retried.
 *
 * @param f - The async function that defines the transaction.
 * @returns A promise resolving to the result of the transaction.
 */
export async function fallibleTransaction<Out, Err>(
  f: (tx: FallibleTransaction<Err>) => Promise<Result<Out, Err>>,
): Promise<TransactionResult<Out, Err>> {
  const guard = markAtomicOperation();
  const tx = new FallibleTransaction<Err>();
  const execute = async () => {
    const result = await f(tx);
    if (result.isOk()) {
      return Result.ok(result.val);
    } else {
      return Result.err(await tx.onFailure(result.val));
    }
  };
  return executeWithDropAsync([guard], execute);
}

/**
 * Extracts the error types from an array of operations.
 *
 * @template T - An array of `Operation` objects.
 * @returns A union type representing the possible error types that can occur in the operations.
 *
 * @example
 * ```typescript
 * const operationOne: Operation<bigint, bigint, string> = operation(
 *   // ...
 * );
 *
 * const operationTwo: Operation<bigint, string, { code: string; message: string }> = operation(
 *   // ...
 * );
 *
 * type Errors = OperationErrors<[typeof operationOne, typeof operationTwo]>;
 * // Errors = string | { code: string; message: string }
 * ```
 *
 */
export type OperationErrors<T extends Operation<unknown, unknown, unknown>[]> = {
  [K in keyof T]: T[K] extends Operation<unknown, unknown, infer Err> ? Err : never;
}[number];
