// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import {
  ToolRpc,
  createStdin,
  createStdout,
  type ByteStreamFailure,
  type ByteStreamItem,
  type FutureInvokeResult,
  type RpcError,
  type ToolError,
} from 'golem:tool/host@0.1.0';
import {
  preflightWitTypedSchemaValue,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  type TypedSchemaValue,
} from '../internal/schema-model';
import {
  mapSettledToolResult,
  settleToolResult,
  toolStreamFailureFromError,
  type SettledToolResult,
  type ToolInputStream,
} from '../internal/tool/startedToolInvocation';

export {
  mapSettledToolResult,
  resultFromSettledToolResult,
  settleToolResult,
  startedToolInvocation,
  ToolStreamError,
  type SettledToolResult,
  type StartedToolInvocation,
  type ToolInputStream,
} from '../internal/tool/startedToolInvocation';

export interface ToolInvocationResult {
  readonly result?: TypedSchemaValue;
}

export interface RawToolInvocation {
  readonly stdout?: AsyncIterable<ByteStreamItem>;
  readonly settledResult: Promise<
    SettledToolResult<Awaited<ReturnType<FutureInvokeResult['get']>>>
  >;
  cancel(): void;
}

export interface ToolClientTransport {
  start(
    commandPath: readonly string[],
    input: Parameters<ToolRpc['asyncInvokeAndAwait']>[1],
    stdin: ToolInputStream | undefined,
    stdout: boolean,
  ): RawToolInvocation;
}

export function createToolClientTransport(toolName: string): ToolClientTransport {
  let rpc: ToolRpc | undefined;
  return {
    start(commandPath, input, stdin, withStdout) {
      rpc ??= new ToolRpc(toolName);
      const inputEndpoints = stdin === undefined ? undefined : createStdin();
      const outputEndpoints = withStdout ? createStdout() : undefined;
      const future = rpc.asyncInvokeAndAwait(
        [...commandPath],
        input,
        inputEndpoints?.[1],
        outputEndpoints?.[0],
      );
      const settledResult = settleToolResult(future.get());
      if (inputEndpoints) void pumpToolStdin(stdin!, inputEndpoints[0], inputEndpoints[2]);
      return {
        stdout: outputEndpoints?.[1],
        settledResult,
        cancel: () => future.cancel(),
      };
    },
  };
}

async function pumpToolStdin(
  source: ToolInputStream,
  writer: ReturnType<typeof createStdin>[0],
  closed: ReturnType<typeof createStdin>[2],
): Promise<void> {
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  try {
    reader = source.getReader();
    const closure = closed.wait().then(() => ({ tag: 'closed' }) as const);
    while (true) {
      const next = await Promise.race([
        reader.read().then((value) => ({ tag: 'next', value }) as const),
        closure,
      ]);
      if (next.tag === 'closed') {
        await reader.cancel();
        return;
      }
      if (next.value.done) {
        await writer.finish();
        return;
      }
      if (!(next.value.value instanceof Uint8Array)) {
        throw new TypeError('tool stdin yielded a non-byte chunk');
      }
      if (next.value.value.byteLength === 0) {
        throw new TypeError('tool stdin yielded an empty chunk');
      }
      await writer.write(next.value.value);
    }
  } catch (error) {
    const reason: ByteStreamFailure = toolStreamFailureFromError(error);
    try {
      await writer.fail(reason);
    } catch {
      // The consumer may have selected the terminal while the source failed.
    }
    try {
      await reader?.cancel(error);
    } catch {
      // Source cancellation is best-effort after the attachment has failed.
    }
  } finally {
    reader?.releaseLock();
  }
}

export interface ToolClientRuntime {
  start(
    commandPath: readonly string[],
    input: TypedSchemaValue,
    stdin: ToolInputStream | undefined,
    stdout: boolean,
  ): {
    stdout?: AsyncIterable<ByteStreamItem>;
    settledResult: Promise<SettledToolResult<ToolInvocationResult>>;
    cancel(): void;
  };
}

export function createToolClientRuntime(
  toolName: string,
  transport: ToolClientTransport = createToolClientTransport(toolName),
): ToolClientRuntime {
  return {
    start(commandPath, input, stdin, stdout) {
      const invocation = transport.start(commandPath, typedSchemaValueToWit(input), stdin, stdout);
      return {
        stdout: invocation.stdout,
        settledResult: mapSettledToolResult(invocation.settledResult, (value) => ({
          result: value.result === undefined ? undefined : typedSchemaValueFromWit(value.result),
        })),
        cancel: () => invocation.cancel(),
      };
    },
  };
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
  for (let i = 0; i < value.length; i++)
    if (!(i in value) || typeof value[i] !== 'string') return false;
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
export function isRpcError(value: unknown): value is RpcError {
  if (!implementationObject(value) || typeof value.tag !== 'string') return false;
  switch (value.tag) {
    case 'cancelled':
      return true;
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
    case 'resource-exhausted':
      return hasOwn(value, 'val') && typeof value.val === 'string';
    case 'remote-tool-error':
      return hasOwn(value, 'val') && isToolError(value.val);
    default:
      return false;
  }
}
export function splitToolRpcError<Declared>(
  error: RpcError,
  decodeCustomError: (payload: TypedSchemaValue) => Declared,
): ToolRuntimeError<Declared> {
  if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error')
    return { tag: 'rpc', error };
  return { tag: 'tool', error: decodeCustomError(typedSchemaValueFromWit(error.val.val)) };
}
export type { RpcError, ToolError };
