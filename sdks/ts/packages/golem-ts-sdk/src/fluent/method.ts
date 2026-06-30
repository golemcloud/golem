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
import { HttpEndpointSpec } from './http';

export type InputRecord = Record<string, StandardSchemaV1>;

/**
 * The structural contract of one method: an input parameter record + a
 * success-value schema. `Output` is a phantom used only for handler inference.
 *
 * The optional metadata fields surface on the WIT `agent-method` record:
 * `description` → `description`, `promptHint` → `prompt-hint`, and `readOnly`
 * (a convenience boolean) → `read-only` (a `no-cache` / no-principal
 * `read-only-config`) when `true`.
 */
export interface MethodSpec<Input extends InputRecord = InputRecord, Output = unknown> {
  readonly input: Input;
  readonly returns: StandardSchemaV1<unknown, Output>;
  /** Human-readable description, surfaced as `agent-method.description`. */
  readonly description?: string;
  /** Optional `prompt-hint`, surfaced as `agent-method.prompt-hint`. */
  readonly promptHint?: string;
  /** When `true`, marks the method as read-only (surfaced as `agent-method.read-only`). */
  readonly readOnly?: boolean;
  /**
   * HTTP endpoint(s) exposing this method. Each is compiled to one
   * `agent-method.http-endpoint` (`http-endpoint-details`) record. Requires the
   * agent to declare a `http` mount on `defineAgent`.
   */
  readonly http?: HttpEndpointSpec | HttpEndpointSpec[];
}

/**
 * Declare an agent method. `input` is a record of Standard Schema values (one
 * per parameter); `returns` is the success-value schema. Use `z.void()` (or the
 * equivalent) for methods with no return value.
 *
 * Optional `description` / `promptHint` / `readOnly` metadata is threaded into
 * the assembled WIT `agent-method`.
 *
 * ```ts
 * method({ input: { by: z.number() }, returns: z.number() })
 * method({ input: {}, returns: z.number(), readOnly: true })
 * ```
 */
export function method<Input extends InputRecord, Output>(spec: {
  input: Input;
  returns: StandardSchemaV1<unknown, Output>;
  description?: string;
  promptHint?: string;
  readOnly?: boolean;
  http?: HttpEndpointSpec | HttpEndpointSpec[];
}): MethodSpec<Input, Output> {
  return {
    input: spec.input,
    returns: spec.returns,
    description: spec.description,
    promptHint: spec.promptHint,
    readOnly: spec.readOnly,
    http: spec.http,
  };
}
