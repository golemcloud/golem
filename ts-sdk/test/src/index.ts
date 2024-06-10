import { GolemItApi } from './interfaces/golem-it-api.js';
import {
    useRetryPolicy,
    executeWithDrop,
    oplogCommit,
    atomically,
    withIdempotenceMode,
    withPersistenceLevel,
    operation,
    Result,
    Operation,
    infallibleTransaction,
} from 'golem-ts';

export const api: typeof GolemItApi = {
    failWithCustomMaxRetries,
    explicitCommit,
    fallibleTransactionTest() {
        return false;
    },
    infallibleTransactionTest,
};

function failWithCustomMaxRetries(maxRetries: number): void {
    const retryPolicy = {
        maxAttempts: maxRetries,
        minDelay: BigInt(1000),
        maxDelay: BigInt(1000),
        multiplier: 1,
    };
    const retry = useRetryPolicy(retryPolicy);
    executeWithDrop([retry], () => {
        throw new Error('Fail now');
    });
}

function explicitCommit(replicas: number): void {
    const now = new Date();
    console.log(`Starting commit with ${replicas} replicas at ${now}`);
    oplogCommit(replicas);
    console.log('Finished commit');
}

function infallibleTransactionTest(): number {
    const result = infallibleTransaction(tx => {
        const result = tx.execute(operationOne, undefined);
        return tx.execute(operationTwo, result)
    });
    return result;
}

const operationOne = operation(
    (_: void) => {
        const random = Math.floor(Math.random() * 10);
        if (random < 4) {
            console.log(`OperationOne error | random ${random}`)
            return Result.err("random was < 4");
        } else {
            console.log(`OperationOne | incrementing input by 1`)
            return Result.ok(random);
        }
    },
    (input, result) => {
      console.log(`Compensating operationOne | input: ${input}, result: ${result}`);
      return Result.unit()
    }
);
  
const operationTwo = operation( 
    (input: number) => {
        if (input < 7) {
            console.log(`OperationTwo error | input ${input}`)
            return Result.err( {
                code: "invalid_num",
                message: "Random number < 7"
            });
        } else {
            return Result.ok(input);
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