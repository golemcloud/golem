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

import type { ToolRpcError } from 'golem:core/types@2.0.0';
import type { TypedSchemaValue } from 'golem:tool/common@0.1.0';
import { createToolClientTransport, isRpcError } from './bridge/tool';
import {
  mapSettledToolResult,
  resultFromSettledToolResult,
  startedToolInvocation,
} from './internal/tool/startedToolInvocation';
import { readConcrete, writeConcrete, type CompiledCommand } from './internal/tool/compiled';
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
  /** Stable leaf registration name when the definition describes an adapted presented surface. */
  readonly lookupName?: string;
}

export type ToolCallErrorCause<Errors> =
  | { readonly tag: 'rpc'; readonly error: ToolRpcError }
  | { readonly tag: 'tool'; readonly error: Errors }
  | {
      readonly tag: 'unknown-error';
      readonly name: string;
      readonly payload: Parameters<typeof decodeDeclaredToolError>[1]['payload'];
    };

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
  const transport =
    options.transport ?? createToolClientTransport(options.lookupName ?? tool.toolName);
  return createToolClient(definition, transport, mapToolClientFailure);
}

/** @internal Compiler-emitted model-free typed tool client. */
export function compiledToolClient(
  toolName: string,
  commands: CompiledCommand[],
  options: ToolClientOptions = {},
): object {
  const transport = options.transport ?? createToolClientTransport(options.lookupName ?? toolName);
  const root: Record<string, unknown> = {};
  for (const command of commands) {
    let target = root;
    const members = command.path.length ? command.path : [toolName];
    for (const member of members.slice(0, -1))
      target = (target[member] ??= {}) as Record<string, unknown>;
    const name = members.at(-1)!;
    const callName = [toolName, ...command.path].join(' ');
    const method = (args: Record<string, unknown>): unknown => {
      const start = () => {
        try {
          if (args === null || typeof args !== 'object' || Array.isArray(args))
            throw new TypeError('tool client arguments must be an object');
          const input = {
            graph: command.input.graph,
            value: writeConcrete(command.input.codec, args),
          };
          const stdin = command.stdin ? args.stdin : undefined;
          if (stdin !== undefined && !(stdin instanceof ReadableStream))
            throw new TypeError('stdin must be a readable stream');
          if (command.stdin?.required && stdin === undefined)
            throw new TypeError('required stdin stream is missing');
          const invocation = transport.start(
            command.path,
            input,
            stdin as ReadableStream<Uint8Array> | undefined,
            command.stdout !== undefined,
          );
          const settled = mapSettledToolResult(
            invocation.settledResult,
            (terminal) => {
              try {
                if (!command.result && terminal.result !== undefined)
                  throw new TypeError('unit command returned an unexpected result');
                if (command.result && terminal.result === undefined)
                  throw new TypeError('structured command result is missing');
                return command.result
                  ? readConcrete(command.result.codec, terminal.result!.value)
                  : undefined;
              } catch (error) {
                throw mapCompiledFailure(error, command, callName);
              }
            },
            (error) => {
              throw mapCompiledFailure(error, command, callName);
            },
          );
          if (!command.stdout) return resultFromSettledToolResult(settled);
          if (!invocation.stdout) throw new TypeError('required stdout stream is missing');
          return startedToolInvocation(invocation.stdout, settled, () => invocation.cancel());
        } catch (error) {
          throw mapCompiledFailure(error, command, callName);
        }
      };
      return command.stdout ? start() : Promise.resolve().then(start);
    };
    Object.defineProperty(target, name, { value: method, enumerable: true });
  }
  return root;
}

function mapCompiledFailure(error: unknown, command: CompiledCommand, callName: string): unknown {
  if (error instanceof ToolCallError) return error;
  if (
    error !== null &&
    typeof error === 'object' &&
    (error as { tag?: unknown }).tag === 'remote-tool-error' &&
    (error as { val?: { tag?: unknown } }).val?.tag === 'custom-error'
  ) {
    const custom = (
      error as {
        val: { val: { name: string; payload: TypedSchemaValue } };
      }
    ).val.val;
    const codec = command.errors[custom.name];
    if (codec) {
      try {
        return new ToolCallError({
          tag: 'tool',
          error: {
            tag: 'err',
            name: custom.name,
            hasPayload: true,
            payload: readConcrete(codec.codec, custom.payload.value),
          },
        });
      } catch (decodeError) {
        return protocolToolCallError(`${callName}: ${errorMessage(decodeError)}`);
      }
    }
    if (Object.prototype.hasOwnProperty.call(command.errors, custom.name))
      return new ToolCallError({
        tag: 'tool',
        error: { tag: 'err', name: custom.name, hasPayload: false },
      });
    return new ToolCallError({ tag: 'unknown-error', name: custom.name, payload: custom.payload });
  }
  if (isRpcError(error)) return new ToolCallError({ tag: 'rpc', error });
  return protocolToolCallError(`${callName}: ${errorMessage(error)}`);
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
  error: ToolRpcError,
  callName: string,
): ToolCallError<unknown> {
  if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error') {
    return new ToolCallError({ tag: 'rpc', error });
  }

  try {
    const declaredError = decodeDeclaredToolError(body, error.val.val, callName);
    return declaredError.tag === 'unknown-error'
      ? new ToolCallError(declaredError)
      : new ToolCallError({ tag: 'tool', error: declaredError });
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
  if (cause.tag === 'unknown-error') {
    return `Remote tool returned unknown declared error "${cause.name}"`;
  }
  return cause.error.tag === 'remote-tool-error'
    ? `Remote tool call failed: ${cause.error.val.tag}`
    : cause.error.tag === 'cancelled'
      ? 'Remote tool call failed: cancelled'
      : `Remote tool call failed: ${cause.error.tag}: ${cause.error.val}`;
}

function isRecord(value: unknown): value is Record<PropertyKey, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
