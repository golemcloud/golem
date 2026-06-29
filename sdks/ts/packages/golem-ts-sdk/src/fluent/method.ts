// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

import { StandardSchemaV1 } from './schema/standardSchema';

export type InputRecord = Record<string, StandardSchemaV1>;

/**
 * The structural contract of one method: an input parameter record + a
 * success-value schema. `Output` is a phantom used only for handler inference.
 */
export interface MethodSpec<Input extends InputRecord = InputRecord, Output = unknown> {
  readonly input: Input;
  readonly returns: StandardSchemaV1<unknown, Output>;
}

/**
 * Declare an agent method. `input` is a record of Standard Schema values (one
 * per parameter); `returns` is the success-value schema. Use `z.void()` (or the
 * equivalent) for methods with no return value.
 *
 * ```ts
 * method({ input: { by: z.number() }, returns: z.number() })
 * method({ input: {}, returns: z.number() })
 * ```
 */
export function method<Input extends InputRecord, Output>(spec: {
  input: Input;
  returns: StandardSchemaV1<unknown, Output>;
}): MethodSpec<Input, Output> {
  return { input: spec.input, returns: spec.returns };
}
