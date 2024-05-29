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
import { markAtomicOperation } from "./guard";
import { Result } from "./result";

/// A unified interface for the different types of transactions. Using it can make the code
/// easier to switch between different transactional guarantees but is more constrained in
/// terms of error types.
export interface Transaction {
  execute<OpIn, OpOut, OpErr>(
    operation: Operation<OpIn, OpOut, OpErr>,
    input: OpIn
  ): Result<OpOut, OpErr>;
  fail<Err>(error: Err): Result<void, Err>;
}

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

export type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

export type TransactionFailure<Err> =
  | { type: "FailedAndRolledBackCompletely"; error: Err }
  | { type: "FailedAndRolledBackPartially"; failure: Err; compensationFailure: Err };

class CompensationAction<Err> {
  constructor(private action: () => Result<void, Err>) {}

  execute(): Result<void, Err> {
    return this.action();
  }
}

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
export function infallibleTransaction<Out, Err>(f: (tx: Transaction) => Out): Out {
  const oplogIndex = getOplogIndex();
  const _atomicRegion = markAtomicOperation();
  const transaction = new InfallibleTransaction(oplogIndex);
  return f(transaction);
}

export class InfallibleTransaction implements Transaction {
  private compensations: CompensationAction<void>[] = [];

  constructor(private beginOplogIndex: OplogIndex) {}

  execute<OpIn, OpOut, OpErr>(
    operation: Operation<OpIn, OpOut, OpErr>,
    input: OpIn
  ): Result<OpOut, OpErr> {
    const result = operation.execute(input);
    if (result.isOk) {
      this.compensations.push(
        new CompensationAction(() => {
          const compensationResult = operation.compensate(input, result.value);
          if (compensationResult.isErr) {
            throw new Error("Compensation action failed");
          }
          return Result.unit()
        })
      );
      return result;
    } else {
      this.retry();
      throw new Error("Unreachable code");
    }
  }

  fail<Err>(error: Err): Result<void, Err> {
    this.retry();
    return Result.err(error);
  }

  retry(): void {
    // Rollback all the compensations in reverse order
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      this.compensations[i].execute();
    }
    setOplogIndex(this.beginOplogIndex);
  }

}

const operationOne = operation(
  (input: number) => {
    if (input < 0) {
      return Result.err("input cannot be negative");
    } else {
      return Result.ok(input + 1);
    }
  },
  (input: number, result: number) => {
    console.log(`Compensating operationOne with input: ${input}, result: ${result}`);
    return Result.unit()
  }
);

const operationTwo = operation( 
  (input: number) => {
    if (input < 0) {
      return Result.err(-1);
    } else {
      return Result.ok(input * 2);
    }
  },
  (input: number, result: number) => {
    console.log(`Compensating operationTwo with input: ${input}, result: ${result}`);
    return Result.unit()
  }
);

function infallibleTransactionTest() {
  const result = infallibleTransaction((tx) => 
     tx
      .execute(operationOne, 1)
      .flatMap(res => tx.execute(operationTwo, res))
  );

  return result;
}

function something() {
  let n = 1;
  const result = n > -1 ? Result.ok(n) : Result.err("n is negative");

  return result.flatMap(val => {
    console.log(val);
    if (val > 0) {
      return Result.ok(val);
    } else {
      return Result.err(-1);
    }
  })
}
  

// export function fallibleTransaction<Out, Err>(
//   f: (tx: FallibleTransaction<Err>) => Result<Out, Err>
// ): TransactionResult<Out, Err> {
//   const transaction = new FallibleTransaction<Err>();
//   const result = f(transaction);
//   if (result.ok) {
//     return Ok(result.val);
//   } else if (result.err) {
//     return Err(transaction.onFail(result.val));
//   }
// }

// export class FallibleTransaction<Err> implements Transaction<Err>{
//   private compensations: CompensationAction<Err>[] = [];

//   execute<OpIn, OpOut>(
//     operation: Operation<OpIn, OpOut, Err>,
//     input: OpIn
//   ): Result<OpOut, Err> {
//     const result = operation.execute(input);
//     if (result.ok) {
//       this.compensations.push(
//         new CompensationAction(() => operation.compensate(input, result.val))
//       );
//     }
//     return result;
//   }

//   fail(failure: Err): TransactionFailure<Err> {
//     for (const compensationAction of this.compensations.slice().reverse()) {
//       const compensationResult = compensationAction.execute();
//       if (compensationResult.err) {
//         return {
//           type: "FailedAndRolledBackPartially",
//           failure,
//           compensationFailure: compensationResult.val,
//         };
//       }
//     }
//     return { type: "FailedAndRolledBackCompletely", error: failure };
//   }
// }


// // TODO: 
// export function transaction<
//   T extends Transaction<any>,
//   Out,
//   OpIn = T extends Transaction<infer I, any, any> ? I : never,
//   OpOut = T extends Transaction<any, infer O, any> ? O : never,
//   Err = T extends Transaction<any, any, infer E> ? E : never
// >(
//   f: (tx: T) => Result<Out, Err>
// ): TransactionResult<Out, Err> {

//   // Function implementation here
// }