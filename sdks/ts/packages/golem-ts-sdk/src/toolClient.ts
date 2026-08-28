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

import type { RpcError } from 'golem:tool/host@0.1.0';
import { createToolClientTransport, isRpcError } from './bridge/tool';
import {
  createToolClient,
  decodeDeclaredToolError,
  getExtendedToolDefinition,
  type AnyToolDefinition,
  type ToolClient,
  type ToolClientFailureContext,
  type ToolClientTransport,
} from './tool';

export interface ToolClientOptions {
  readonly transport?: ToolClientTransport;
}

export type ToolCallErrorCause<Errors> =
  | { readonly tag: 'rpc'; readonly error: RpcError }
  | { readonly tag: 'tool'; readonly error: Errors };

/** A stable rejected-promise error for remote tool calls. */
export class ToolCallError<Errors = never> extends Error {
  readonly cause: ToolCallErrorCause<Errors>;

  constructor(cause: ToolCallErrorCause<Errors>) {
    super(formatToolCallError(cause));
    this.name = 'ToolCallError';
    this.cause = cause;
  }
}

/** Assemble a typed runtime client using the ambient tool host by default. */
export function client<Definition extends AnyToolDefinition>(
  definition: Definition,
  options: ToolClientOptions = {},
): ToolClient<Definition> {
  const tool = getExtendedToolDefinition(definition);
  const transport = options.transport ?? createToolClientTransport(tool.toolName);
  return createToolClient(definition, transport, mapToolClientFailure);
}

function mapToolClientFailure(
  error: unknown,
  { body, callName }: ToolClientFailureContext,
): ToolCallError<unknown> {
  if (error instanceof ToolCallError) return error;
  if (isRpcError(error)) return mapToolRpcError(body, error, callName);
  return protocolToolCallError(`${callName}: ${errorMessage(error)}`);
}

function mapToolRpcError(
  body: ToolClientFailureContext['body'],
  error: RpcError,
  callName: string,
): ToolCallError<unknown> {
  if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error') {
    return new ToolCallError({ tag: 'rpc', error });
  }

  try {
    const declaredError = decodeDeclaredToolError(body, error.val.val, callName);
    return new ToolCallError({ tag: 'tool', error: declaredError });
  } catch (decodeError) {
    if (decodeError instanceof ToolCallError) return decodeError;
    return protocolToolCallError(`${callName}: ${errorMessage(decodeError)}`);
  }
}

function protocolToolCallError(message: string): ToolCallError<never> {
  return new ToolCallError<never>({
    tag: 'rpc',
    error: { tag: 'protocol-error', val: message },
  });
}

function formatToolCallError(cause: ToolCallErrorCause<unknown>): string {
  if (cause.tag === 'tool') {
    const name = isRecord(cause.error) ? cause.error.name : undefined;
    return typeof name === 'string'
      ? `Remote tool returned declared error "${name}"`
      : 'Remote tool returned a declared error';
  }
  return cause.error.tag === 'remote-tool-error'
    ? `Remote tool call failed: ${cause.error.val.tag}`
    : `Remote tool call failed: ${cause.error.tag}: ${cause.error.val}`;
}

function isRecord(value: unknown): value is Record<PropertyKey, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
