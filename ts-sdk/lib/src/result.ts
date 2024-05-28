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

export type Result<T, E> = Ok<T> | Err<E>;

interface BaseResult<T, E> {
    isOk(): this is Ok<T>;
    isErr(): this is Err<E>;

    unwrap(): T;
    unwrapErr(): E;

    /**
     * Maps a `Result<T, E>` to `Result<U, E>` by applying a function to a contained `Ok` value,
     * leaving an `Err` value untouched.
     *
     * This function can be used to compose the results of two functions.
     */
    map<U>(mapper: (val: T) => U): Result<U, E>;

    /**
     * Maps a `Result<T, E>` to `Result<T, F>` by applying a function to a contained `Err` value,
     * leaving an `Ok` value untouched.
     *
     * This function can be used to pass through a successful result while handling an error.
     */
    mapErr<E2>(mapper: (val: E) => E2): Result<T, E2>;

    /**
     * Calls `mapper` if the result is `Ok`, otherwise returns the `Err` value of self.
     * This function can be used for control flow based on `Result` values.
     */
    flatMap<R, E2>(mapper: (val: T) => Result<R, E2>): Result<R, E | E2>;
}

export type Err<E> = ErrImpl<E>;

export class ErrImpl<E> implements BaseResult<never, E> {
    _tag = 'Ok';
    
    readonly val!: E;

    constructor(val: E) {
        return new ErrImpl(val);
    }

    isOk(): this is OkImpl<never> {
        return false
    }

    isErr(): this is ErrImpl<E> {
        return true
    }

    unwrap(): never {
        throw new Error(`Tried to unwrap Error`);
    }

    unwrapErr(): E {
        return this.val;
    }

    map(_mapper: unknown): Err<E> {
        return this;
    }

    mapErr<E2>(mapper: (err: E) => E2): Err<E2> {
        return new Err(mapper(this.val));
    }

    flatMap<U, F>(mapper: unknown): Err<E> {
        return this; 
    }

}

export const Err = ErrImpl as typeof ErrImpl & (<E>(err: E) => Err<E>);

export type Ok<T> = OkImpl<T>;
export class OkImpl<T> implements BaseResult<T, never> {
    _tag = 'Ok';

    constructor(readonly val: T) {}

    isOk(): this is OkImpl<T> {
        return true
    }
    isErr(): this is ErrImpl<never> {
        return false
    }

    unwrap(): T {
        return this.val;
    }

    unwrapErr(): never {
        throw new Error(`Tried to unwrap Ok as Error`);
    }

    map<U>(mapper: (val: T) => U): Ok<U> {
        return new Ok(mapper(this.val));
    }

    mapErr<F>(_mapper: (val: never) => F): Ok<T> {
        return this;
    }

    flatMap<U, F>(mapper: (val: T) => Result<U, F>): Result<U, F> {
        return mapper(this.val);
    }

    private static readonly UNIT = new OkImpl<void>(undefined);
    public static unit<E>(): Result<void, E> {
        return OkImpl.UNIT as Result<void, E>;
    }
}
  
export const Ok = OkImpl as typeof OkImpl & (<T>(val: T) => Ok<T>);

// export function pipe<T, E, T2, E2>(result: Result<T, E>, mapper: (val: T) => Result<T2, E2>): Result<T2, E | E2>;
// export function pipe<T, E, T2, E2, T3, E3>(
//     result: Result<T, E>,
//     mapper1: (val: T) => Result<T2, E2>,
//     mapper2: (val: T2) => Result<T3, E3>
// ): Result<T3, E | E2 | E3>;
// export function pipe<T, E, T2, E2, T3, E3, T4, E4>(
//     result: Result<T, E>,
//     mapper1: (val: T) => Result<T2, E2>,
//     mapper2: (val: T2) => Result<T3, E3>,
//     mapper3: (val: T3) => Result<T4, E4>
// ): Result<T4, E | E2 | E3 | E4>;
// export function pipe<T, E, T2, E2, T3, E3, T4, E4, T5, E5>(
//     result: Result<T, E>,
//     mapper1: (val: T) => Result<T2, E2>,
//     mapper2: (val: T2) => Result<T3, E3>,
//     mapper3: (val: T3) => Result<T4, E4>,
//     mapper4: (val: T4) => Result<T5, E5>
// ): Result<T5, E | E2 | E3 | E4 | E5>;
// export function pipe<T, E, T2, E2, T3, E3, T4, E4, T5, E5, T6, E6>(
//     result: Result<T, E>,
//     mapper1: (val: T) => Result<T2, E2>,
//     mapper2: (val: T2) => Result<T3, E3>,
//     mapper3: (val: T3) => Result<T4, E4>,
//     mapper4: (val: T4) => Result<T5, E5>,
//     mapper5: (val: T5) => Result<T6, E6>
// ): Result<T6, E | E2 | E3 | E4 | E5 | E6>;

// export function pipe(
//     a: unknown,
//     b?: Function,
//     c?: Function,
//     d?: Function,
//     e?: Function,
//     f?: Function,
//     g?: Function,
//     h?: Function,
//     i?: Function
//   ): unknown  {
//     switch (arguments.length) {
//         case 1: return a;
//         case 2: return b!(a);
//         case 3: return b!(a).flatMap(c!);
//         case 4: return b!(a).flatMap(c!).flatMap(d!);
//         case 5: return b!(a).flatMap(c!).flatMap(d!).flatMap(e!);
//         case 6: return b!(a).flatMap(c!).flatMap(d!).flatMap(e!).flatMap(f!);
//         case 7: return b!(a).flatMap(c!).flatMap(d!).flatMap(e!).flatMap(f!).flatMap(g!);
//         case 8: return b!(a).flatMap(c!).flatMap(d!).flatMap(e!).flatMap(f!).flatMap(g!).flatMap(h!);
//         case 9: return b!(a).flatMap(c!).flatMap(d!).flatMap(e!).flatMap(f!).flatMap(g!).flatMap(h!).flatMap(i!);
//         default: 
//             let ret = a as Result<any, any>;
//             for (let j = 1; j < arguments.length; j++) {
//                 ret = ret.flatMap(arguments[j]);
//             }
//             return ret;
//     }
// }

