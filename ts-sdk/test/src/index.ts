import { fallibleTransaction, infallibleTransaction, Operation, operation, OperationErrors, Result } from 'golem-ts';
import { GolemTsApi } from './interfaces/golem-ts-api.js';

export const api: typeof GolemTsApi  = {
    process: (a: bigint) =>  {
        const result = infallibleTransaction(tx => {
            const resultA = tx.execute(operationOne, a);
            const resultB = resultA.flatMap(a => tx.execute(operationTwo, a));
            return resultB
        });
        if (result.isOk) {
            return result.value;
        } else {
            console.log(`Error: ${result.error}`);
            return "Error";
        }
    },
    processFallible: (a: bigint) =>  {
        type Error = OperationErrors<[typeof neverFailsOperation, typeof alwaysFailsOperation]>;
        const result = fallibleTransaction<bigint, Error>(tx => {
            const resultA = tx.execute(neverFailsOperation, a);
            const resultB = resultA.flatMap(num => tx.execute(alwaysFailsOperation, num));
            return resultB;
          });

        if (result.isOk) {
            return result.value.toString();
        } else {
            const message = typeof result.error === "string" ? result.error : JSON.stringify(result.error);
            return `Error ${message}`;
        }
    }
}

const operationOne: Operation<bigint, bigint, string> = operation(
    (input: bigint) => {
        const random = Math.floor(Math.random() * 10);
        if (random < 5) {
            console.log(`OperationOne | input: ${input} | random ${random} | negative input detected`)
            return Result.err("input cannot be negative");
        } else {
            console.log(`OperationOne | incrementing input by 1`)
            return Result.ok(input + BigInt(1));
        }
    },
    (input, result) => {
      console.log(`Compensating operationOne | input: ${input}, result: ${result}`);
      return Result.unit()
    }
);
  
const operationTwo = operation( 
    (input: bigint) => {
        const random = Math.floor(Math.random() * 10);
        if (random < 8) {
            console.log(`OperationTwo | input: ${input} | input too large`)
            return Result.err( {
                code: "invalid_random",
                message: "random number not = 9"
            });
        } else {
            console.log(`OperationTwo | input: ${input} | converting input to string`)
            return Result.ok("Valid BigInt: " + input.toString());
        }
    },
    (input, result) => {
        console.log(`Compensating operationTwo | input: ${input}, result: ${result}`);
        return Result.unit()
    }
);

const neverFailsOperation = operation<bigint, bigint, string>( 
    (input: bigint) => {
        console.log(`SuccessOperation | input: ${input}`);
        return Result.ok(input);
    }, 
    (_input, _result) => {
        console.log("neverFailsOperation");
        return Result.unit()
    }
)

const alwaysFailsOperation = operation<bigint, bigint, string>( (input: bigint) => {
    console.log(`FailOperation | input: ${input}`);
        return Result.err("Always fails");
    },
    (_input, _result) => {
        console.log("alwaysFailsOperation");
        return Result.unit()
    }
)