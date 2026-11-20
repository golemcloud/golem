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

import {
  AccountPrincipal,
  AgentPrincipal,
  AnonymousPrincipal,
  Caller,
  TokenProvenance,
  ToolPrincipal,
  UserPrincipal,
  type Principal,
} from './caller';
import type { Invalid } from './httpTypes';

// A Principal, a Caller and a TokenProvenance are live values: the host tells an invocation who
// called, and that cannot travel as method data. Using one in a method input, in a result or
// inside a value type is rejected at build time.

/** The message of every Rule 8 failure. */
export type LiveValueMessage =
  'Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.caller (the InvocationContext argument), and store ctx.caller.original.ref as a PrincipalRef.';

type LiveValue = Principal | Caller | TokenProvenance;

type IsAny<T> = 0 extends 1 & T ? true : false;

// `true` when `T` is, or contains in an object field, an array element, a map or set value, or a
// union member, a live value. The depth bound keeps recursive value types finite.
type Contains<T, Depth extends readonly unknown[] = []> = Depth['length'] extends 6
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
            : T extends readonly (infer E)[]
              ? Contains<E, [...Depth, 0]>
              : T extends object
                ? { [K in keyof T]-?: Contains<T[K], [...Depth, 0]> }[keyof T]
                : never;

/** `true` when a live value can be reached from `T`. */
export type ContainsLiveValue<T> = [Contains<T>] extends [never] ? false : true;

/**
 * Resolves to `unknown` when none of `Types` can reach a live value, and to an
 * {@link Invalid} carrier that names the rule otherwise. Intersect it with a parameter.
 */
export type RejectLiveValues<Types> = true extends {
  [K in keyof Types]: ContainsLiveValue<Types[K]>;
}[keyof Types]
  ? Invalid<LiveValueMessage>
  : unknown;

export const LIVE_VALUE_MESSAGE: LiveValueMessage =
  'Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.caller (the InvocationContext argument), and store ctx.caller.original.ref as a PrincipalRef.';

/** Throws the Rule 8 message for a live value that reaches a codec at run time. */
export function assertNotLiveValue(value: unknown, depth = 0): void {
  if (value === null || typeof value !== 'object' || depth > 6) return;
  if (isLiveValueInstance(value)) {
    throw new TypeError(LIVE_VALUE_MESSAGE);
  }
  if (Array.isArray(value)) {
    for (const element of value) assertNotLiveValue(element, depth + 1);
  } else if (value instanceof Map) {
    for (const [key, entry] of value) {
      assertNotLiveValue(key, depth + 1);
      assertNotLiveValue(entry, depth + 1);
    }
  } else if (value instanceof Set) {
    for (const element of value) assertNotLiveValue(element, depth + 1);
  } else if (Object.getPrototypeOf(value) === Object.prototype) {
    for (const entry of Object.values(value)) assertNotLiveValue(entry, depth + 1);
  }
}

function isLiveValueInstance(value: object): boolean {
  return (
    value instanceof Caller ||
    value instanceof TokenProvenance ||
    value instanceof UserPrincipal ||
    value instanceof AccountPrincipal ||
    value instanceof AgentPrincipal ||
    value instanceof ToolPrincipal ||
    value instanceof AnonymousPrincipal
  );
}
