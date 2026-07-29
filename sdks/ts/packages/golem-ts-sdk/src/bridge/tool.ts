// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { ToolRpc, type RpcError, type ToolError } from 'golem:tool/host@0.1.0';
import {
  preflightWitTypedSchemaValue,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  type TypedSchemaValue,
} from '../internal/schema-model';

export interface ToolInvocationResult {
  readonly result?: TypedSchemaValue;
  readonly stdout?: AsyncIterable<number>;
}
export interface ToolClientTransport {
  invokeAndAwait(
    commandPath: readonly string[],
    input: Parameters<ToolRpc['invokeAndAwait']>[1],
    stdin: AsyncIterable<number> | undefined,
  ):
    | Awaited<ReturnType<ToolRpc['invokeAndAwait']>>
    | Promise<Awaited<ReturnType<ToolRpc['invokeAndAwait']>>>;
}

export function createToolClientTransport(toolName: string): ToolClientTransport {
  let rpc: ToolRpc | undefined;
  return {
    invokeAndAwait(commandPath, input, stdin) {
      rpc ??= new ToolRpc(toolName);
      return rpc.invokeAndAwait([...commandPath], input, stdin);
    },
  };
}

export interface ToolClientRuntime {
  invokeAndAwait(
    commandPath: readonly string[],
    input: TypedSchemaValue,
    stdin?: AsyncIterable<number>,
  ): Promise<ToolInvocationResult>;
}

export function createToolClientRuntime(
  toolName: string,
  transport: ToolClientTransport = createToolClientTransport(toolName),
): ToolClientRuntime {
  return {
    async invokeAndAwait(commandPath, input, stdin) {
      const result = await transport.invokeAndAwait(
        commandPath,
        typedSchemaValueToWit(input),
        stdin,
      );
      try {
        return {
          result: result.result === undefined ? undefined : typedSchemaValueFromWit(result.result),
          stdout: result.stdout,
        };
      } catch (error) {
        await closeAsyncIterable(result.stdout);
        throw error;
      }
    },
  };
}

/** Close a returned stdout iterator when generated post-invocation validation fails. */
export async function disposeToolStdout(stdout: AsyncIterable<number> | undefined): Promise<void> {
  await closeAsyncIterable(stdout);
}

async function closeAsyncIterable(value: AsyncIterable<number> | undefined): Promise<void> {
  try {
    await value?.[Symbol.asyncIterator]().return?.();
  } catch {
    // Stream cleanup is best-effort when response validation fails.
  }
}

export type ToolRuntimeError<Declared> =
  | { readonly tag: 'rpc'; readonly error: RpcError }
  | { readonly tag: 'tool'; readonly error: Declared };

function implementationObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function hasOwn(value: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function isDenseStringList(value: unknown): value is string[] {
  if (!Array.isArray(value)) return false;
  for (let i = 0; i < value.length; i++) {
    if (!(i in value) || typeof value[i] !== 'string') return false;
  }
  return true;
}

function isTypedSchemaValue(value: unknown): boolean {
  try {
    preflightWitTypedSchemaValue(value as Parameters<typeof preflightWitTypedSchemaValue>[0]);
    return true;
  } catch {
    return false;
  }
}

function isToolError(value: unknown): value is ToolError {
  if (!implementationObject(value) || typeof value.tag !== 'string' || !hasOwn(value, 'val'))
    return false;
  switch (value.tag) {
    case 'invalid-tool-name':
    case 'invalid-input':
    case 'constraint-violation':
    case 'invalid-result':
      return typeof value.val === 'string';
    case 'invalid-command-path':
      return isDenseStringList(value.val);
    case 'custom-error':
      return isTypedSchemaValue(value.val);
    default:
      return false;
  }
}

/**
 * Recognize host errors with a non-consuming decode-safety preflight. This is
 * not complete sanitization of fabricated ABI-impossible nested objects; a
 * successful result promises the unchanged payload is decodable at this trust
 * boundary.
 */
export function isRpcError(value: unknown): value is RpcError {
  if (!implementationObject(value) || typeof value.tag !== 'string' || !hasOwn(value, 'val'))
    return false;
  switch (value.tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
      return typeof value.val === 'string';
    case 'remote-tool-error':
      return isToolError(value.val);
    default:
      return false;
  }
}

/** Split a host RPC failure from a declared custom tool error without requiring a ToolDefinition. */
export function splitToolRpcError<Declared>(
  error: RpcError,
  decodeCustomError: (payload: TypedSchemaValue) => Declared,
): ToolRuntimeError<Declared> {
  if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error')
    return { tag: 'rpc', error };
  return { tag: 'tool', error: decodeCustomError(typedSchemaValueFromWit(error.val.val)) };
}

export type { RpcError, ToolError };
