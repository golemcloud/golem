const prototype = {
  /**
   * Returns `this.value` if `this` is a successful result, otherwise throws a TypeError.
   * @example Returns the payload of a successful result.
   * Result.ok(123).unwrap() // 123
   * @example Throws the payload of a failed result.
   * Result.err('error').unwrap() // throws 'error'
   */
  unwrap,

  /**
   * 
   * Returns `this.error` if `this` is a failed result, otherwise throws a TypeError.
   * @example Returns the payload of a failed result.
   * Result.
   */
  unwrapErr,
  /**
   * Returns the payload of the result.
   * @example Returns the payload of a successful result.
   * Result.ok(123).toUnion() // 123
   * @example Returns the payload of a failed result.
   * Result.err('error').toUnion() // 'error'
   */
  toUnion,
  /**
   * Return the result of applying one of the given functions to the payload.
   * @example
   * Result.ok(123).match((x) => x * 2, (x: string) => x + '!') // 246
   * Result.err('error').match((x: number) => x * 2, (x) => x + '!') // 'error!'
   */
  match,
  /**
   * Creates a Result value by modifying the payload of the successful result using the given function.
   * @example
   * Result.ok(123).map((x) => x * 2) // Result.ok(246)
   * Result.err('error').map((x: number) => x * 2) // Result.err('error')
   */
  map,
  /**
   * Creates a Result value by modifying the payload of the failed result using the given function.
   * @example
   * Result.ok(123).mapError((x: string) => x + '!') // Result.ok(123)
   * Result.err('error').mapError((x) => x + '!') // Result.err('error!')
   */
  mapError,
  /**
   * Calls the given function with the payload of the result and returns the result unchanged.
   */
  tap,
  /**
   * Maps the payload of the successful result and flattens the nested Result type.
   * @example
   * Result.ok(123).flatMap((x) => Result.ok(x * 2)) // Result.ok(246)
   * Result.ok(123).flatMap((x) => Result.err('error')) // Result.err('error')
   * Result.err('error').flatMap((x: number) => Result.ok(x * 2)) // Result.err('error')
   * Result.err('error').flatMap((x) => Result.err('failure')) // Result.err('error')
   */
  flatMap,
  /**
   * Flattens the nested Result type.
   * @example
   * Result.ok(Result.ok(123)).flatten() // Result.ok(123)
   * Result.ok(Result.err('error')).flatten() // Result.err('error')
   * Result.err('error').flatten() // Result.err('error')
   */
  flatten,
  /**
   * Perform a safe cast of the error type to the given class. If the payload of the failed result is not instance of constructor, throws TypeError.
   * @example
   * const result: Result<number, Error> = Result.tryCatch(() => {
   *   if (Math.random() >= 0) {
   *     throw new Error('error')
   *   } else {
   *     return 123
   *   }
   * }).assertErrorInstanceOf(Error)
   */
  assertErrorInstanceOf,
} as const

/** Type representing success or failure. */
export type Result<T, E = unknown> = Result.Ok<T> | Result.Err<E>

export namespace Result {
  /**
   * The type of a successful result.
   * @example
   * const success: Result.ok<number> = Result.ok(123)
   */
  export type Ok<T> = typeof prototype & {
    readonly value: T
    readonly error?: never
    readonly isOk: true
    readonly isErr: false
  }

  /**
   * A failed result.
   * @example
   * const failure: Result.err<string> = Result.err('error')
   */
  export type Err<E> = typeof prototype & {
    readonly value?: never
    readonly error: E
    readonly isOk: false
    readonly isErr: true
  }

  const UNIT = ok(undefined) 

  /**
   * @returns A successful result with no payload.
   */
  export function unit(): Ok<void> {
    return UNIT 
  }

  /**
   * Create a successful result.
   */
  export function ok<T>(value: T): Result.Ok<T> {
    return withPrototype({ value, isOk: true, isErr: false }, prototype)
  }

  /**
   * Create an error result.
   */
  export function err<E>(error: E): Err<E> {
    return withPrototype({ error, isOk: false, isErr: true }, prototype)
  }

  /**
   * 
   */
  export function tryCatch<T>(f: () => T): Result<T, unknown> {
    try {
      return ok(f())
    } catch (error) {
      return err(error)
    }
  }

  export function fromNullish(value: null): Result.Err<null>
  export function fromNullish(value: undefined): Result.Err<undefined>
  export function fromNullish(value: null | undefined): Result.Err<null | undefined>
  export function fromNullish<T extends {}>(value: T): Result.Ok<T>
  export function fromNullish<T>(value: T | null): Result<T, null>
  export function fromNullish<T>(value: T | undefined): Result<T, undefined>
  export function fromNullish<T>(value: T | null | undefined): Result<T, null | undefined>
  export function fromNullish<T>(value: T | null | undefined) {
    return value != null ? Result.ok(value) : Result.err(value)
  }

  export function all<T>(results: readonly Result.Ok<T>[]): Result.Ok<T[]>
  export function all<T, E>(results: readonly Result<T, E>[]): Result<T[], E>
  export function all<T, E>(results: readonly Result<T, E>[]): Result<T[], E> {
    const values: T[] = []
    for (const result of results) {
      if (result.isErr) return result
      values.push(result.value)
    }
    return Result.ok(values)
  }

}

function withPrototype<T, P extends {}>(target: T, prototype: P): T & Omit<P, keyof T> {
  return Object.assign(Object.create(prototype), target)
}

function unwrap<T>(this: Result.Ok<T>): T
function unwrap<E>(this: Result.Err<E>): never
function unwrap<T, E>(this: Result<T, E>): T
function unwrap<T, E>(this: Result<T, E>): T {
  if (this.isOk) return this.value
  else throw new TypeError('unwrap called on Err result: ' + this.error) 
}

function unwrapErr<T>(this: Result.Ok<T>): never
function unwrapErr<E>(this: Result.Err<E>): E
function unwrapErr<T, E>(this: Result<T, E>): E
function unwrapErr<T, E>(this: Result<T, E>): E {
  if (this.isOk) throw new TypeError('unwrapErr called on Ok result: ' + this.value)
  else return this.error
}

function toUnion<T>(this: Result.Ok<T>): T
function toUnion<E>(this: Result.Err<E>): E
function toUnion<T, E>(this: Result<T, E>): T | E
function toUnion<T, E>(this: Result<T, E>): T | E {
  if (this.isOk) return this.value
  else return this.error
}

function match<T, E, T2, E2>(this: Result.Ok<T>, f: (value: T) => T2, g: (error: E) => E2): T2
function match<T, E, T2, E2>(this: Result.Err<E>, f: (value: T) => T2, g: (error: E) => E2): E2
function match<T, E, T2, E2>(this: Result<T, E>, f: (value: T) => T2, g: (error: E) => E2): T2 | E2
function match<T, E, T2, E2>(
  this: Result<T, E>,
  f: (value: T) => T2,
  g: (error: E) => E2,
): T2 | E2 {
  if (this.isOk) return f(this.value)
  else return g(this.error)
}

function map<T, T2>(this: Result.Ok<T>, f: (value: T) => T2): Result.Ok<T2>
function map<T, E, T2>(this: Result.Err<E>, f: (value: T) => T2): Result.Err<E>
function map<T, E, T2>(this: Result<T, E>, f: (value: T) => T2): Result<T2, E>
function map<T, E, T2>(this: Result<T, E>, f: (value: T) => T2): Result<T2, E> {
  if (this.isErr) return this
  else return Result.ok(f(this.value))
}

function mapError<T, E, E2>(this: Result.Ok<T>, f: (error: E) => E2): Result.Ok<T>
function mapError<E, E2>(this: Result.Err<E>, f: (error: E) => E2): Result.Err<E2>
function mapError<T, E, E2>(this: Result<T, E>, f: (error: E) => E2): Result<T, E2>
function mapError<T, E, E2>(this: Result<T, E>, f: (error: E) => E2): Result<T, E2> {
  if (this.isOk) return this
  else return Result.err(f(this.error))
}

function tap<E>(this: Result.Err<E>, f: (error: E) => void): Result.Err<E>
function tap<T>(this: Result.Ok<T>, f: (value: T) => void): Result.Ok<T>
function tap<T, E>(this: Result<T, E>, f: (value: T) => void): Result<T, E>
function tap<T, E>(this: Result<T, E>, f: (value: T) => void): Result<T, E> {
  if (this.isOk) f(this.value)
  return this
}

function flatMap<T, T2>(
  this: Result.Ok<T>,
  f: (value: T) => Result.Ok<T2>,
): Result.Ok<T2>
function flatMap<T, E2>(
  this: Result.Ok<T>,
  f: (value: T) => Result.Err<E2>,
): Result.Err<E2>
function flatMap<T, T2, E2>(
  this: Result.Ok<T>,
  f: (value: T) => Result<T2, E2>,
): Result<T2, E2>
function flatMap<T, E, T2, E2>(
  this: Result.Err<E>,
  f: (value: T) => Result<T2, E2>,
): Result.Err<E>
function flatMap<T, E, T2>(this: Result<T, E>, f: (value: T) => Result.Ok<T2>): Result<T2, E>
function flatMap<T, E, T2, E2>(
  this: Result<T, E>,
  f: (value: T) => Result.Err<E2>,
): Result.Err<E | E2>
function flatMap<T, E, T2, E2>(
  this: Result<T, E>,
  f: (value: T) => Result<T2, E2>,
): Result<T2, E | E2>
function flatMap<T, E, T2, E2>(this: Result<T, E>, f: (value: T) => Result<T2, E2>) {
  if (this.isErr) return this
  else return f(this.value)
}

function flatten<E>(this: Result.Err<E>): Result.Err<E>
function flatten<E>(this: Result.Ok<Result.Err<E>>): Result.Err<E>
function flatten<T>(this: Result.Ok<Result.Ok<T>>): Result.Ok<T>
function flatten<T, E>(this: Result.Ok<Result<T, E>>): Result<T, E>
function flatten<T, E>(this: Result<Result.Ok<T>, E>): Result<T, E>
function flatten<E, E2>(this: Result<Result.Err<E>, E2>): Result.Err<E | E2>
function flatten<T, E, E2>(this: Result<Result<T, E>, E2>): Result<T, E | E2>
function flatten<T, E, E2>(this: Result<Result<T, E>, E2>): Result<T, E | E2> {
  if (this.isErr) return this
  else return this.value
}

function assertErrorInstanceOf<T, C extends abstract new (..._: any) => any>(
  this: Result.Ok<T>,
  constructor: C,
): Result.Ok<T>
function assertErrorInstanceOf<E, C extends abstract new (..._: any) => any>(
  this: Result.Err<E>,
  constructor: C,
): Result.Err<E & InstanceType<C>>
function assertErrorInstanceOf<T, E, C extends abstract new (..._: any) => any>(
  this: Result<T, E>,
  constructor: C,
): Result<T, E & InstanceType<C>>
function assertErrorInstanceOf<T, E, C extends abstract new (..._: any) => any>(
  this: Result<T, E>,
  constructor: C,
): Result<T, E & InstanceType<C>> {
  if (this.isOk) return this

  if (this.error instanceof constructor) return this as any

  throw new TypeError(`Assertion failed: Expected error to be an instance of ${constructor.name}.`)
}


// function pipe<T, E, T2, E2>(
//   this: Result<T, E>,
//   f: (value: T) => Result<T2, E2>,
// ): Result<T2, E2>;
// function pipe<T, E, T2, E2, T3, E3>(
//   this: Result<T, E>,
//   f: (value: T) => Result<T2, E2>,
//   g: (value: T2) => Result<T3, E3>,
// ): Result<T3, E3>;
