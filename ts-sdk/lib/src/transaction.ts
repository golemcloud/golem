// Copyright 2024 Golem Cloud
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

import { getOplogIndex, setOplogIndex, OplogIndex } from "./bindgen/bindgen";
import { executeWithDrop, markAtomicOperation } from "./guard";
import { Result } from "./result";

export interface Operation<In, Out, Err> {
  execute(input: In): Result<Out, Err>;
  compensate(input: In, result: Out): Result<void, Err>;
}

export function operation<In, Out, Err>(
  execute: (input: In) => Result<Out, Err>,
  compensate: (input: In, result: Out) => Result<void, Err>
): Operation<In, Out, Err> {
  return new OperationImpl(execute, compensate);
}

class OperationImpl<In, Out, Err> implements Operation<In, Out, Err> {
  constructor(
    public readonly execute: (input: In) => Result<Out, Err>,
    public readonly compensate: (input: In, result: Out) => Result<void, Err>
  ) {}
}

class InfallibleTransaction {
  private compensations: (() => void) [] = [];

  constructor(
    private readonly beginOplogIndex: OplogIndex
  ) {}

  execute<In, Out, Err>(
    operation: Operation<In, Out, Err>,
    input: In,
  ) : Result<Out, Err> {
    const result = operation.execute(input);
    if (result.isOk) {
      this.compensations.push(
        // Compensations cannot fail in infallible transactions.
        () => {
          const compensationResult = operation.compensate(input, result.value);
          if (compensationResult.isErr) {
            throw new Error("Compensation action failed");
          }
        }
      );
      return Result.ok(result.value);
    } else {
      this.retry();
      throw new Error("Unreachable code");
    }
  }

  private retry(): void {
    console.log("Executing compensations");
    // Rollback all the compensations in reverse order
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      console.log("Compensating operation ", i)
      try {
        this.compensations[i]();
      } catch (e) {
        console.log("Compensation failed", e)
        throw e;
      }
      console.log("Compensated operation ", i)
    }
    console.log("Retrying transaction from ", this.beginOplogIndex);
    setOplogIndex(this.beginOplogIndex);
  }
}


class FallibleTransaction<Err> {
  private compensations: (() => Result<void, Err>) [] = [];

  constructor() {}

  execute<In, Out, OpErr extends Err>(
    operation: Operation<In, Out, OpErr>,
    input: In,
  ) : Result<Out, Err> {
    const result = operation.execute(input);
    if (result.isOk) {
      this.compensations.push(
        () => {
          return operation.compensate(input, result.value) 
        }
      );
      return  result;
    } else {
      return result;
    }
  }

  onFailure(error: Err): TransactionFailure<Err>{
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      const compensationResult = this.compensations[i]();
      if (compensationResult.isErr) {
        return {
          type: "FailedAndRolledBackPartially",
          error,
          compensationFailure: compensationResult.error
        }
      }
    }
    return {
      type: "FailedAndRolledBackCompletely",
      error 
    }
  }
}


export type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

export type TransactionFailure<Err> =
  | { type: "FailedAndRolledBackCompletely"; error: Err }
  | { type: "FailedAndRolledBackPartially"; error: Err; compensationFailure: Err };

/// InfallibleTransaction is a sequence of operations that are executed in a way that if any of the
/// operations or the underlying Golem executor fails, the whole transaction is going to
/// be retried.
///
/// In addition to that, **user level failures** (represented by the `Result::Err` value
/// of an operation) lead to performing the compensation actions of each already performed operation
/// in reverse order.
///
/// Fatal errors (panic) and external executor failures are currently cannot perform the
/// rollback actions.
export function infallibleTransaction<Out>(f: (tx: InfallibleTransaction) => Out) : Out {
  const guard = markAtomicOperation();
  const beginOplogIndex = getOplogIndex();
  const tx = new InfallibleTransaction(beginOplogIndex);
  return executeWithDrop([guard], () => f(tx));
}

export function fallibleTransaction<Out, Err>(f: (tx: FallibleTransaction<Err>) => Result<Out, Err>) : TransactionResult<Out, Err> {
  const guard = markAtomicOperation();
  const tx = new FallibleTransaction<Err>();
  const execute = () => {
    let result = f(tx);
    if (result.isOk){
      return Result.ok(result.value);
    } else {
      return Result.err(tx.onFailure(result.error));
    }
  };
  return executeWithDrop([guard], execute);
}

type ExtractOperationError<T> = T extends Operation<any, any, infer Err> ? Err : never;

type UnionOfOperationErrors<T extends (Operation<any, any, any> | any)[]> = T[number] extends Operation<any, any, infer Err>
  ? Err
: NonOperationTypes<T>;

type NonOperationTypes<T extends (Operation<any, any, any> | any)[]> = T[number] extends Operation<any, any, any>
  ? never
  : T[number];

type ExtractOperationOutput<T> = T extends Operation<any, infer Out, any> ? Out : never;

type LastOperationOutput<T extends (Operation<any, any, any> | any)[]> = T extends [...any[], infer Last]
  ? Last extends Operation<any, any, any>
    ? ExtractOperationOutput<Last>
    : never
  : never;

export function fallibleTransactionOperations<
  Operations extends any[],
  Out = LastOperationOutput<Operations>
>(
  f: (tx: FallibleTransaction<UnionOfOperationErrors<Operations>>) => Result<Out, UnionOfOperationErrors<Operations>>
): TransactionResult<Out, UnionOfOperationErrors<Operations>> {
  return fallibleTransaction(f);
}