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

import { getSelfMetadata } from './hostapi';
import { nextRetryDelay, resolveRetryPolicy, type PredicateValue } from 'golem:api/retry@1.5.0';
import { now } from 'wasi:clocks/system-clock@0.3.0';
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

class RetrySaga extends Error {
  constructor(readonly original: unknown) {
    super('Saga operation failed');
  }
}

/** A saga stopped after compensation, exhaustion, or an invalid retry policy. */
export type SagaRetryFailureReason =
  | { type: 'exhausted' }
  | { type: 'compensation-failure'; compensationFailure: unknown }
  | { type: 'policy-error'; policyError: unknown };

export class SagaRetryFailure extends Error {
  constructor(
    readonly error: unknown,
    readonly reason: SagaRetryFailureReason,
  ) {
    super('Saga failed');
  }
}

function clock(): bigint {
  const instant = now();
  return instant.seconds * 1_000_000_000n + BigInt(instant.nanoseconds);
}

class InfallibleSaga {
  private compensations: (() => Promise<void>)[] = [];
  private terminal: { error: unknown } | undefined;

  checkActive(): void {
    if (this.terminal) throw this.terminal.error;
  }

  failure(error: unknown): unknown {
    return this.terminal ? this.terminal.error : error;
  }

  async compensate(error: unknown): Promise<void> {
    const actions = this.compensations;
    this.compensations = [];
    for (let i = actions.length - 1; i >= 0; i--) {
      try {
        await actions[i]();
      } catch (compensationFailure) {
        throw new SagaRetryFailure(error, { type: 'compensation-failure', compensationFailure });
      }
    }
  }

  /**
   * Executes a compensable step within the infallible saga.
   * @param step - The step to execute.
   * @param input - The input to the step.
   * @returns A promise resolving to the result of the step.
   */
  async execute<In, Out, Err>(step: Compensable<In, Out, Err>, input: In): Promise<Out> {
    this.checkActive();
    const result = await step.execute(input);
    if (result.isOk()) {
      this.compensations.push(async () => {
        const compensationResult = await step.compensate(input, result.val);
        if (compensationResult.isErr()) {
          throw compensationResult.val;
        }
      });
      return result.val;
    } else {
      return this.retry(result.val);
    }
  }

  private async retry(error: unknown): Promise<never> {
    this.terminal = { error: new RetrySaga(error) };
    // Rollback all the compensations in reverse order
    const compensations = this.compensations;
    this.compensations = [];
    try {
      for (let i = compensations.length - 1; i >= 0; i--) {
        await compensations[i]();
      }
    } catch (error) {
      this.terminal = {
        error: new SagaRetryFailure((this.terminal.error as RetrySaga).original, {
          type: 'compensation-failure',
          compensationFailure: error,
        }),
      };
    }
    throw this.terminal.error;
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
      let compensationResult: Result<void, Err>;
      try {
        compensationResult = await this.compensations[i]();
      } catch (compensationFailure) {
        return { type: 'FailedAndRolledBackPartially', error, compensationFailure };
      }
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
      error: unknown;
      compensationFailure: unknown;
    };

/**
 * Executes an infallible saga.
 *
 * An infallible saga retries user-level failures after compensation. Each attempt appends
 * new durable calls without restoring JavaScript state. Compensate local state explicitly.
 * Do not reuse an external operation's idempotency key after compensating it.
 *
 * In addition, **user level failures** (represented by the `Result::Err` value of a step)
 * lead to performing the compensation actions of each already performed step in reverse order.
 *
 * Caught exceptions also run compensation before the host retry decision.
 *
 * @param f - The async function that defines the saga.
 * @returns A promise resolving to the result of the saga.
 */
export async function infallibleSaga<Out>(
  f: (saga: InfallibleSaga) => Promise<Out>,
): Promise<Result<Out, SagaRetryFailure>> {
  const started = clock();
  let state: Uint8Array = new Uint8Array();
  let attempt = 0n;
  while (true) {
    const saga = new InfallibleSaga();
    try {
      const result = await f(saga);
      saga.checkActive();
      return Result.ok(result);
    } catch (error) {
      // A caught compensation failure must not become a retry or a success.
      const e = saga.failure(error);
      const original = e instanceof RetrySaga ? e.original : e;
      try {
        await saga.compensate(original);
      } catch (failure) {
        return Result.err(failure as SagaRetryFailure);
      }
      if (e instanceof SagaRetryFailure) return Result.err(e);
      try {
        const id = getSelfMetadata().agentId;
        const noun = `urn:worker:${id.componentId}/${id.agentId}`;
        const properties: [string, PredicateValue][] = [
          ['attempt', { tag: 'integer', val: ++attempt }],
          [
            'error-kind',
            {
              tag: 'text',
              val:
                e instanceof RetrySaga
                  ? 'operation-error'
                  : e instanceof Error && e.name === 'AbortError'
                    ? 'interrupted'
                    : 'exception',
            },
          ],
        ];
        const policy = resolveRetryPolicy('saga', noun, properties);
        const elapsed = clock() - started;
        const decision = nextRetryDelay(
          policy,
          state,
          elapsed < 0n ? 0n : elapsed,
          'saga',
          noun,
          properties,
        );
        state = decision.state;
        if (decision.delay === undefined)
          return Result.err(new SagaRetryFailure(original, { type: 'exhausted' }));
        let milliseconds = (decision.delay + 999_999n) / 1_000_000n;
        while (milliseconds > 0n) {
          const chunk = Number(milliseconds > 2_147_483_647n ? 2_147_483_647n : milliseconds);
          await new Promise<void>((resolve) => globalThis.setTimeout(resolve, chunk));
          milliseconds -= BigInt(chunk);
        }
      } catch (policyError) {
        return Result.err(new SagaRetryFailure(original, { type: 'policy-error', policyError }));
      }
    }
  }
}

/**
 * Executes a fallible saga.
 *
 * A fallible saga is a sequence of compensable steps. When the saga callback returns
 * `Result::Err`, compensation runs for successfully completed steps in reverse order.
 * If a compensation returns `Result::Err`, compensation stops and the saga reports
 * `FailedAndRolledBackPartially`.
 *
 * Caught exceptions run compensation and are rethrown unchanged when rollback completes.
 *
 * @param f - The async function that defines the saga.
 * @returns A promise resolving to the result of the saga.
 */
export async function fallibleSaga<Out, Err>(
  f: (saga: FallibleSaga<Err>) => Promise<Result<Out, Err>>,
): Promise<SagaResult<Out, Err>> {
  const saga = new FallibleSaga<Err>();
  try {
    const result = await f(saga);
    let out: SagaResult<Out, Err>;
    if (result.isOk()) {
      out = Result.ok(result.val);
    } else {
      out = Result.err(await saga.onFailure(result.val));
    }
    return out;
  } catch (e) {
    const failure = await saga.onFailure(e as Err);
    if (failure.type === 'FailedAndRolledBackPartially') return Result.err(failure);
    throw e;
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
