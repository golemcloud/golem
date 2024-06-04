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
export interface Transaction<Err>{
  /**
   * Execute an operation from the context of the transaction. 
   */
  execute<OpIn, OpOut>(
    operation: Operation<OpIn, OpOut, Err>,
    input: OpIn
  ): Result<OpOut, Err>;

  /**
   * Fail the entire transaction with the given error. This will attempt to rollback all the operations
   */
  fail(error: Err): Result<void, Err>;

  // fn run<Out>(f: impl FnOnce(&mut Self) -> Result<Out, Err>) -> TransactionResult<Out, Err>;
  run<Out>(f: (tx: Transaction<Err>) => Result<Out, Err>): TransactionResult<Out, Err>;
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

// class InfallibleTransactionState<In, Err> {
//   private compensations: (() => void) [] = [];
//   private err: boolean = false;

//   constructor(
//     public readonly value: In,
//     private readonly beginOplogIndex: OplogIndex
//   ) {}

//   execute<Out, Err2>(
//     operation: Operation<In, Out, Err2>
//   ): Result<Out, Err2>
//    {
//     if (this.err) {
//       throw new Error("Transaction is in a failed state");
//     }
//     const result = operation.execute(this.value);
//     if (result.isOk) {
//       this.compensations.push(() => {
//         const compensationResult = operation.compensate(this.value, result.value);
//         if (compensationResult.isErr) {
//           throw new Error("Compensation action failed " + compensationResult.error);
//         }
//       })
//       return result;
//     } else {
//       this.err = true;
//       this.retry();
//       throw new Error("Unreachable");
//     }
//   }

//   retry(): void {
//     // Rollback all the compensations in reverse order
//     for (let i = this.compensations.length - 1; i >= 0; i--) {
//       this.compensations[i]();
//     }
//     setOplogIndex(this.beginOplogIndex);
//   }

// }


class InfallibleTransactionState {
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

  retry(): void {
    // Rollback all the compensations in reverse order
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      this.compensations[i]();
    }
    setOplogIndex(this.beginOplogIndex);
  }
}

class InfallibleTransaction<In, Out, Err> {
  private operations: Operation<any, any, any>[] = [];
  private compensations: (() => void) [] = [];

  constructor(
    operation: Operation<In, Out, Err>,
  ) {
    this.operations.push(operation);
  }

  next<NextOut, NextErr>(operation: Operation<Out, NextOut, NextErr>): InfallibleTransaction<In, NextOut, Err | NextErr> {
    console.log("Adding new operation to the transaction")
    this.operations.push(operation);
    return this as unknown as InfallibleTransaction<In, NextOut, Err | NextErr>;
  }

  execute(input: In): Result<Out, Err> {
    let index = getOplogIndex();
    using _atomic = markAtomicOperation();

    console.log(`Executing transaction with ${this.operations.length} steps`)
    let currentInput: any = input;
    let currentOutput: any;

    for (const operation of this.operations) {
      const result = operation.execute(currentInput);
      if (result.isOk) {
        console.log("Operation succeeded")
        this.compensations.push(() => {
          const compensationResult = operation.compensate(input, result.value);
          if (compensationResult.isErr) {
            throw new Error("Compensation action failed");
          }
          return Result.unit()
        });
        currentInput = result.value;
        currentOutput = result.value;
      } else {
        console.log("Operation failed")
        this.retry(index)
        throw new Error("Unreachable code")
      }
    }

    console.log("Transaction completed successfully")

    return Result.ok(currentOutput) as Result<Out, Err>;
  }

  private retry(index: OplogIndex): void {
    console.log("Retrying transaction")
    // Rollback all the compensations in reverse order
    for (let i = this.compensations.length - 1; i >= 0; i--) {
      this.compensations[i]();
    }
    console.log("Setting oplog index to the beginning", index);
    setOplogIndex(index);
  }
}

export type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

export type TransactionFailure<Err> =
  | { type: "FailedAndRolledBackCompletely"; error: Err }
  | { type: "FailedAndRolledBackPartially"; failure: Err; compensationFailure: Err };

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
// export function infallibleTransaction<Err>(): Transaction<Err>{
//   return new InfallibleTransaction(
//     getOplogIndex(),
//     markAtomicOperation()
//   );
// }
export function infallibleTransaction<In, Out, Err>(
  operation: Operation<In, Out, Err>
): InfallibleTransaction<In, Out, Err> {
  console.log("Creating new transaction")
  return new InfallibleTransaction(
    operation, 
  );
}

export function infallibleTransaction2<Out>(f: (tx: InfallibleTransactionState) => Out) : Out {
  using _atomic = markAtomicOperation();
  const beginOplogIndex = getOplogIndex();
  const tx = new InfallibleTransactionState(beginOplogIndex);
  const result = f(tx);
  return result
}

// function testInfallible() {
//   let res = infallibleTransaction2(tx => {
//     let res = tx.execute(operationOne, 1);

//   })
// }

// export class InfallibleTransaction<Err> implements Transaction<Err> {
//   private compensations: CompensationAction<void>[] = [];

//   constructor(
//     private beginOplogIndex: OplogIndex,
//     private atomicRegion: AtomicOperationGuard
//   ) {}

//   execute<OpIn, OpOut>(
//     operation: Operation<OpIn, OpOut, Err>,
//     input: OpIn
//   ): Result<OpOut, Err> {
//     const result = operation.execute(input);
//     if (result.isOk) {
//       this.compensations.push(
//         // Compensations cannot fail in infallible transactions.
//         new CompensationAction(() => {
//           const compensationResult = operation.compensate(input, result.value);
//           if (compensationResult.isErr) {
//             throw new Error("Compensation action failed");
//           }
//           return Result.unit()
//         })
//       );
//       return result;
//     } else {
//       this.retry();
//       throw new Error("Unreachable code");
//     }
//   }

//   fail<Err>(error: Err): Result<void, Err> {
//     this.retry();
//     return Result.err(error);
//   }

//   run<Out>(f: (tx: Transaction<Err>) => Result<Out, Err>): TransactionResult<Out, Err> {
//     return Result.ok(f(this).unwrap())
//   }

//   retry(): void {
//     // Rollback all the compensations in reverse order
//     for (let i = this.compensations.length - 1; i >= 0; i--) {
//       this.compensations[i].execute();
//     }
//     setOplogIndex(this.beginOplogIndex);
//   }
// }


// export class FallibleTransaction<Err> implements Transaction<Err> {
//   private compensations: CompensationAction<Err>[] = [];

//   constructor(
//     private beginOplogIndex: OplogIndex,
//     private atomicRegion: AtomicOperationGuard
//   ) {}

//   execute<OpIn, OpOut>(
//     operation: Operation<OpIn, OpOut, Err>,
//     input: OpIn
//   ): Result<OpOut, Err> {
//     const result = operation.execute(input);
//     if (result.isOk) {
//       this.compensations.push(
//         new CompensationAction(() => 
//           operation.compensate(input, result.value)
//         )
//       );
//     } 
//     return result;
//   }

//   fail(error: Err): Result<void, Err> {
//     throw new Error("Method not implemented.");
//   }

// }



// function infallibleTransactionTest() {
//   let tx = infallibleTransaction<string | number>();
//   let result = tx.execute(operationOne, 1).flatMap(res => tx.execute(operationTwo, res));

//   return result;
// }

// function seqTransaction() {
//   let tx = infallibleTransaction(operationOne).next(operationTwo);
//   let result = tx.execute(1);
//   return result;
// }

// function something() {
//   let n = 1;
//   const result = n > -1 ? Result.ok(n) : Result.err("n is negative");

//   return result.flatMap(val => {
//     console.log(val);
//     if (val > 0) {
//       return Result.ok(val);
//     } else {
//       return Result.err(-1);
//     }
//   })
// }
  

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


