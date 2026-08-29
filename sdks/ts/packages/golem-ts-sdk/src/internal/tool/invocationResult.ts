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

import type { TypedSchemaValue } from 'golem:tool/common@0.1.0';
import { sourceValueIsCanonical, type SchemaCodec } from '../../schema/codec';
import { t, typedSchemaValueToWit, v } from '../schema-model';
import type { ExtendedErrorCase } from './model';
import { schemaValueConforms } from './validation';

export interface DeclaredToolError {
  readonly tag: 'err';
  readonly name: string;
  readonly hasPayload: boolean;
  readonly payload?: unknown;
}

export function isDeclaredToolError(value: unknown): value is DeclaredToolError {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const error = value as Record<string, unknown>;
  return (
    error.tag === 'err' && typeof error.name === 'string' && typeof error.hasPayload === 'boolean'
  );
}

export function encodeToolValue(
  codec: SchemaCodec,
  value: unknown,
  position: string,
): TypedSchemaValue {
  try {
    const encoded = codec.toValue(value);
    if (!schemaValueConforms(codec.graph, codec.graph.root, encoded)) {
      throw new Error('does not match its declared schema');
    }
    if (!sourceValueIsCanonical(codec, value, encoded)) {
      throw new Error('is not canonical for its declared schema');
    }
    return typedSchemaValueToWit({ graph: codec.graph, value: encoded });
  } catch (error) {
    throw invalidToolResult(`${position}: ${errorMessage(error)}`);
  }
}

export function invalidToolResult(message: string) {
  return { tag: 'invalid-result' as const, val: message };
}

export function encodeDeclaredToolErrorPayload(
  errorCase: ExtendedErrorCase,
  error: DeclaredToolError,
  errorLabel: string,
  payloadPosition = `${errorLabel} payload`,
): TypedSchemaValue {
  const hasPayloadProperty = Object.prototype.hasOwnProperty.call(error, 'payload');
  if (!errorCase.payloadCodec) {
    if (error.hasPayload || hasPayloadProperty) {
      throw invalidToolResult(`${errorLabel} does not declare a payload`);
    }
    return typedSchemaValueToWit({
      graph: { defs: new Map(), root: t.tuple([]) },
      value: v.tuple([]),
    });
  }
  if (!error.hasPayload || !hasPayloadProperty) {
    throw invalidToolResult(`${errorLabel} requires a payload`);
  }
  return encodeToolValue(errorCase.payloadCodec, error.payload, payloadPosition);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
