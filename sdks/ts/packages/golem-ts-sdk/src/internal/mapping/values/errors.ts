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

import * as util from 'node:util';
import { Value } from './Value';
import { NameOptionTypePair } from '../types/analysedType';

// type mismatch in tsValue when converting from TS to WIT
export function typeMismatchInSerialize(tsValue: any, expectedType: string): string {
  return `Type mismatch. Expected type \`${safeDisplay(expectedType)}\`, got \`${safeDisplay(tsValue)}\``;
}

export function customSerializationError(message: string): string {
  return `Internal error: ${message}`;
}

// Unable to convert the value to the expected type in the output direction
export function typeMismatchInDeserialize(value: Value, expectedType: string) {
  return `Failed to deserialize the following internal value to typescript type \`${expectedType}\`: \`${safeDisplay(value)}\``;
}

// Missing keys in tsValue when converting from TS to WIT
export function missingObjectKey(key: string, tsValue: any): string {
  return `Missing key '${key}' in ${safeDisplay(tsValue)}`;
}

// tsValue does not match any of the union types when converting from TS to WIT
export function unionTypeMatchError(unionTypes: NameOptionTypePair[], tsValue: any): string {
  const types = unionTypes.map((t) => t.name);
  return `Value '${safeDisplay(tsValue)}' does not match any of the union types: ${types.join(', ')}`;
}

export function enumMismatchInSerialize(enumValues: string[], tsValue: any): string {
  return `Value '${safeDisplay(tsValue)}' does not match any of the enum values: ${enumValues.join(', ')}`;
}

// unhandled type of tsValue when converting from TS to WIT
export function unhandledTypeError(
  tsValue: any,
  typeName: string | undefined,
  message: string | undefined,
): string {
  const error =
    `${safeDisplay(tsValue)}` +
    (typeName ? ` inferred as ${typeName}` : '') +
    ` cannot be handled. `;
  return error + (message ? `${message}` : '');
}

export function safeDisplay(tsValue: any): string {
  return util.format(tsValue);
}
