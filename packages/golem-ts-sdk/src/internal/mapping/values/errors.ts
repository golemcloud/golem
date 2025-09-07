// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Option from '../../../newTypes/option';

// type mismatch in tsValue when converting from TS to WIT
export function typeMismatchIn(tsValue: any, expectedType: Type.Type): string {
  const nameOrKind = expectedType.name ?? expectedType.kind;
  return `Type mismatch. Expected ${nameOrKind}, but got ${safeDisplay(tsValue)} which is of type ${typeof tsValue}`;
}

// Missing keys in tsValue when converting from TS to WIT
export function missingValueForKey(key: string, tsValue: any): string {
  return `Missing key '${key}' in ${safeDisplay(tsValue)}`;
}

// tsValue does not match any of the union types when converting from TS to WIT
export function unionTypeMatchError(
  unionTypes: Type.Type[],
  tsValue: any,
): string {
  return `Value '${safeDisplay(tsValue)}' does not match any of the union types: ${unionTypes.map((t) => t.name).join(', ')}`;
}

// unhandled type of tsValue when converting from TS to WIT
export function unhandledTypeError(
  tsValue: any,
  typeName: Option.Option<string>,
  message: Option.Option<string>,
): string {
  const error =
    `${safeDisplay(tsValue)}` +
    (Option.isSome(typeName) ? ` inferred as ${typeName.val}` : '') +
    ` cannot be handled. `;
  return error + (Option.isSome(message) ? `${message.val}` : '');
}

// Unable to convert the value to the expected type in the output direction
export function typeMismatchOut(value: any, expectedType: string) {
  return "Unable to convert '" + safeDisplay(value) + "' to " + expectedType;
}

// A best effort to display any value.
// We return the original value only as a last resort in error messages.
export function safeDisplay(tsValue: any): any {
  try {
    return JSON.stringify(tsValue);
  } catch {
    try {
      return String(tsValue);
    } catch {
      return tsValue;
    }
  }
}
