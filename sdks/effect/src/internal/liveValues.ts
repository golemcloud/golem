/**
 * Rule 8: a Principal, a Caller and a TokenProvenance are live values. The host tells an invocation
 * who called, and that cannot travel as method data, so a signature, a result, an agent id or a
 * value type that can reach one is rejected when the program is type-checked.
 *
 * @internal
 * @since 2.0.0
 */
import type { Caller, Principal, TokenProvenance } from "../InvocationContext.js"
import type { Invalid } from "./httpTypes.js"

/**
 * The message of every Rule 8 failure.
 *
 * @internal
 * @since 2.0.0
 */
export type LiveValueMessage =
  "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.caller (the InvocationContext service), and store ctx.caller.original.ref as a PrincipalRef."

type LiveValue = Principal | Caller | TokenProvenance

type IsAny<T> = 0 extends 1 & T ? true : false

// `true` when `T` is, or contains in an object field, an array element, a map or set value, or a
// union member, a live value. The depth bound keeps recursive value types finite.
type Contains<T, Depth extends ReadonlyArray<unknown> = []> = Depth["length"] extends 6
  ? never
  : IsAny<T> extends true
    ? never
    : T extends LiveValue
      ? true
      : T extends (...args: never[]) => unknown
        ? never
        : T extends ReadonlyMap<infer K, infer V>
          ? Contains<K, [...Depth, 0]> | Contains<V, [...Depth, 0]>
          : T extends ReadonlySet<infer E>
            ? Contains<E, [...Depth, 0]>
            : T extends ReadonlyArray<infer E>
              ? Contains<E, [...Depth, 0]>
              : T extends object
                ? { [K in keyof T]-?: Contains<T[K], [...Depth, 0]> }[keyof T]
                : never

/**
 * `true` when a live value can be reached from `T`.
 *
 * @internal
 * @since 2.0.0
 */
export type ContainsLiveValue<T> = [Contains<T>] extends [never] ? false : true

/**
 * Resolves to `unknown` when none of `Types` can reach a live value, and to an {@link Invalid}
 * carrier that names the rule otherwise. Intersect it with a parameter.
 *
 * @internal
 * @since 2.0.0
 */
export type RejectLiveValues<Types> = true extends {
  [K in keyof Types]: ContainsLiveValue<Types[K]>
}[keyof Types]
  ? Invalid<LiveValueMessage>
  : unknown
