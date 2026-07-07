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

import { markAtomicOperation } from './guard';
import { type OplogIndex, getOplogIndex, setOplogIndex, trap } from './hostapi';
import { Result } from './result';

/**
 * A compensable step of a saga: an action with a matching rollback (compensation).
 *
 * Implement this interface and use it within a saga block (`fallibleSaga` /
 * `infallibleSaga`). Steps can also be constructed from closures via `compensable`.
 */
export interface Compensable<In, Out, Err> {
  /**
   * The action to execute.
   * @param input - The input to the step.
   * @returns A promise resolving to the result of the step.
   */
  execute(input: In): Promise<Result<Out, Err>>;

  /**
   * Compensation to perform in case of failure.
   * Compensations should not throw errors.
   * @param input - The input to the step.
   * @param result - The result of the step.
   * @returns A promise resolving to the result of the compensation.
   */
  compensate(input: In, result: Out): Promise<Result<void, Err>>;
}

/**
 * Creates a {@link Compensable} step from the provided execute and compensate functions.
 * @param execute - The function to execute the step.
 * @param compensate - The function to compensate the step in case of failure.
 * @returns The created compensable step.
 */
export function compensable<In, Out, Err>(
  execute: (input: In) => Promise<Result<Out, Err>>,
  compensate: (input: In, result: Out) => Promise<Result<void, Err>>,
): Compensable<In, Out, Err> {
  return new CompensableImpl(execute, compensate);
}

class CompensableImpl<In, Out, Err> implements Compensable<In, Out, Err> {
  constructor(
    public readonly execute: (input: In) => Promise<Result<Out, Err>>,
    public readonly compensate: (input: In, result: Out) => Promise<Result<void, Err>>,
  ) {}
}

class InfallibleSaga {
  private compensations: (() => Promise<void>)[] = [];

  constructor(private readonly beginOplogIndex: OplogIndex) {}

  /**
   * Executes a compensable step within the infallible saga.
   * @param step - The step to execute.
   * @param input - The input to the step.
   * @returns A promise resolving to the result of the step.
   */
  async execute<In, Out, Err>(step: Compensable<In, Out, Err>, input: In): Promise<Out> {
    const result = await step.execute(input);
    if (result.isOk()) {
      this.compensations.push(
        // Compensations cannot fail in an infallible saga.
        async () => {
          const compensationResult = await step.compensate(input, result.val);
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

class FallibleSaga<Err> {
  private compensations: (() => Promise<Result<void, Err>>)[] = [];

  /**
   * Executes a compensable step within the fallible saga.
   * @param step - The step to execute.
   * @param input - The input to the step.
   * @returns A promise resolving to the result of the step.
   */
  async execute<In, Out, OpErr extends Err>(
    step: Compensable<In, Out, OpErr>,
    input: In,
  ): Promise<Result<Out, Err>> {
    const result = await step.execute(input);
    if (result.isOk()) {
      this.compensations.push(async () => {
        return await step.compensate(input, result.val);
      });
      return result;
    } else {
      return result;
    }
  }

  /**
   * Handles the failure of the fallible saga.
   * @param error - The error that caused the failure.
   * @returns A promise resolving to the saga failure result.
   */
  async onFailure(error: Err): Promise<SagaFailure<Err>> {
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

export type SagaResult<Out, Err> = Result<Out, SagaFailure<Err>>;

export type SagaFailure<Err> =
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
 * Executes an infallible saga.
 *
 * An infallible saga is a sequence of compensable steps executed such that if any of the
 * steps or the underlying Golem executor fails, the whole saga is retried.
 *
 * In addition, **user level failures** (represented by the `Result::Err` value of a step)
 * lead to performing the compensation actions of each already performed step in reverse order.
 *
 * Fatal errors (panic) and external executor failures currently cannot perform the
 * rollback actions.
 *
 * @param f - The async function that defines the saga.
 * @returns A promise resolving to the result of the saga.
 */
export async function infallibleSaga<Out>(f: (saga: InfallibleSaga) => Promise<Out>): Promise<Out> {
  const guard = markAtomicOperation();
  const beginOplogIndex = getOplogIndex();
  const saga = new InfallibleSaga(beginOplogIndex);
  try {
    const result = await f(saga);
    guard.drop();
    return result;
  } catch (e) {
    // Force an uncatchable trap so user code cannot observe the failure with
    // a `try/catch`. The atomic region is intentionally left open; the
    // existing replay-time fallback in `mark_begin_operation` deletes the
    // partial inner side effects and re-executes the block.
    trap(`infallibleSaga failed: ${formatErrorForTrap(e)}`);
    throw e;
  }
}

/**
 * Executes a fallible saga.
 *
 * A fallible saga is a sequence of compensable steps executed such that if any of the
 * steps fails, all the already performed steps' compensation actions get executed in
 * reverse order.
 *
 * In case of fatal errors (panic) and external executor failures, it does not perform the
 * compensation actions and the whole saga gets retried.
 *
 * @param f - The async function that defines the saga.
 * @returns A promise resolving to the result of the saga.
 */
export async function fallibleSaga<Out, Err>(
  f: (saga: FallibleSaga<Err>) => Promise<Result<Out, Err>>,
): Promise<SagaResult<Out, Err>> {
  const guard = markAtomicOperation();
  const saga = new FallibleSaga<Err>();
  try {
    const result = await f(saga);
    let out: SagaResult<Out, Err>;
    if (result.isOk()) {
      out = Result.ok(result.val);
    } else {
      out = Result.err(await saga.onFailure(result.val));
    }
    guard.drop();
    return out;
  } catch (e) {
    // Force an uncatchable trap. Note: this only fires for *thrown* errors,
    // which are unexpected. Expected failures are returned via Result.err
    // and processed by `saga.onFailure` above without trapping.
    trap(`fallibleSaga failed: ${formatErrorForTrap(e)}`);
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

/**
 * Extracts the error types from an array of compensable steps.
 *
 * @template T - An array of `Compensable` steps.
 * @returns A union type representing the possible error types that can occur in the steps.
 *
 * @example
 * ```typescript
 * const stepOne: Compensable<bigint, bigint, string> = compensable(
 *   // ...
 * );
 *
 * const stepTwo: Compensable<bigint, string, { code: string; message: string }> = compensable(
 *   // ...
 * );
 *
 * type Errors = CompensableErrors<[typeof stepOne, typeof stepTwo]>;
 * // Errors = string | { code: string; message: string }
 * ```
 *
 */
export type CompensableErrors<T extends Compensable<unknown, unknown, unknown>[]> = {
  [K in keyof T]: T[K] extends Compensable<unknown, unknown, infer Err> ? Err : never;
}[number];
