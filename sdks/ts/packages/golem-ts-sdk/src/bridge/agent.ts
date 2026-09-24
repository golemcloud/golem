// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { makeAgentId, WasmRpc } from 'golem:agent/host@2.0.0';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import type {
  CancellationToken,
  CancelableScheduledInvocationReceipt,
  Datetime,
  InvocationMetadata,
  RpcError,
  ScheduledInvocationReceipt,
} from 'golem:agent/host@2.0.0';
import { awaitAbortable, throwIfAborted } from '../internal/pollableUtils';
import {
  schemaValueFromWit,
  schemaValueToWit,
  schemaValueToWitAsync,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  type SchemaValue,
  type TypedSchemaValue,
} from '../internal/schema-model';
import type { Uuid } from '../uuid';

export type RemoteAgentError<Value = TypedSchemaValue> =
  | { readonly tag: 'invalid-input'; readonly details: string }
  | { readonly tag: 'invalid-method'; readonly details: string }
  | { readonly tag: 'invalid-type'; readonly details: string }
  | { readonly tag: 'invalid-agent-id'; readonly details: string }
  | { readonly tag: 'custom-error'; readonly value: Value };

export type RemoteCallErrorCause<Value = TypedSchemaValue> =
  | { readonly tag: 'protocol-error'; readonly details: string }
  | { readonly tag: 'denied'; readonly details: string }
  | { readonly tag: 'not-found'; readonly details: string }
  | { readonly tag: 'remote-internal-error'; readonly details: string }
  | { readonly tag: 'remote-agent-error'; readonly error: RemoteAgentError<Value> };

export class RemoteCallError<Value = TypedSchemaValue> extends Error {
  readonly _tag = 'RemoteCallError';
  override readonly cause: RemoteCallErrorCause<Value>;

  constructor(context: string, cause: RemoteCallErrorCause<Value>) {
    super(`${context}: ${formatRemoteCallErrorCause(cause)}`, { cause });
    this.name = 'RemoteCallError';
    this.cause = cause;
  }
}

export function isRemoteCallError(error: unknown): error is RemoteCallError {
  return (
    typeof error === 'object' &&
    error !== null &&
    (error as { _tag?: unknown })._tag === 'RemoteCallError' &&
    typeof (error as { message?: unknown }).message === 'string' &&
    isRemoteCallErrorCause((error as { cause?: unknown }).cause)
  );
}

export class RemoteOutputError extends Error {
  readonly _tag = 'RemoteOutputError';

  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = 'RemoteOutputError';
  }
}
export interface RemoteInvocationResult<Value = SchemaValue> {
  metadata: InvocationMetadata;
  value?: Value;
}
export interface AgentConfigEntry {
  readonly path: readonly string[];
  readonly value: TypedSchemaValue;
}

type RemoteAgentCreation = (
  agentTypeName: string,
  constructorTree: ReturnType<typeof schemaValueToWit>,
  phantomId: Uuid | undefined,
  config: Array<{ path: string[]; value: ReturnType<typeof typedSchemaValueToWit> }>,
  agentId: string,
) => WasmRpc;

function isRpcError(error: unknown): error is RpcError {
  if (error === null || typeof error !== 'object') return false;

  switch ((error as { tag?: unknown }).tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
    case 'remote-agent-error':
      return true;
    default:
      return false;
  }
}

function remoteCallError(context: string, error: RpcError): RemoteCallError {
  return new RemoteCallError(context, mapRemoteCallErrorCause(error, typedSchemaValueFromWit));
}

function mapRemoteCallErrorCause<Value>(
  error: RpcError,
  decode: (value: Parameters<typeof typedSchemaValueFromWit>[0]) => Value,
): RemoteCallErrorCause<Value> {
  switch (error.tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
      return { tag: error.tag, details: error.val };
    case 'remote-agent-error':
      return { tag: error.tag, error: mapRemoteAgentError(error.val, decode) };
  }
}

function isRemoteCallErrorCause(cause: unknown): cause is RemoteCallErrorCause {
  if (typeof cause !== 'object' || cause === null) return false;
  const tagged = cause as { tag?: unknown; details?: unknown; error?: unknown };
  switch (tagged.tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
      return typeof tagged.details === 'string';
    case 'remote-agent-error':
      return isRemoteAgentError(tagged.error);
    default:
      return false;
  }
}

function isRemoteAgentError(error: unknown): error is RemoteAgentError {
  if (typeof error !== 'object' || error === null) return false;
  const tagged = error as { tag?: unknown; details?: unknown; value?: unknown };
  switch (tagged.tag) {
    case 'invalid-input':
    case 'invalid-method':
    case 'invalid-type':
    case 'invalid-agent-id':
      return typeof tagged.details === 'string';
    case 'custom-error':
      return typeof tagged.value === 'object' && tagged.value !== null;
    default:
      return false;
  }
}

function mapRemoteAgentError<Value>(
  error: Extract<RpcError, { tag: 'remote-agent-error' }>['val'],
  decode: (value: Parameters<typeof typedSchemaValueFromWit>[0]) => Value,
): RemoteAgentError<Value> {
  switch (error.tag) {
    case 'invalid-input':
    case 'invalid-method':
    case 'invalid-type':
    case 'invalid-agent-id':
      return { tag: error.tag, details: error.val };
    case 'custom-error':
      return { tag: error.tag, value: decode(error.val) };
  }
}

function formatRemoteCallErrorCause(cause: RemoteCallErrorCause<unknown>): string {
  if (cause.tag !== 'remote-agent-error') return `${cause.tag}: ${cause.details}`;
  return cause.error.tag === 'custom-error'
    ? 'remote-agent-error: custom-error'
    : `remote-agent-error: ${cause.error.tag}: ${cause.error.details}`;
}

function mapRpcError<T>(context: string, operation: () => T): T {
  try {
    return operation();
  } catch (error) {
    if (!isRpcError(error)) throw error;
    throw remoteCallError(context, error);
  }
}

function disposeOwnedWitResources(tree: SchemaValueTree): void {
  for (const node of tree.valueNodes) {
    switch (node.tag) {
      case 'stream-value':
      case 'secret-value':
      case 'quota-token-handle':
      case 'permission-card-handle':
        try {
          // Native lowering spends the wrapper handle. Disposal releases only
          // resources still owned here, including after partial lowering.
          const resource = node.val as { [Symbol.dispose]?: () => void };
          resource[Symbol.dispose]?.();
        } catch {
          // Attempt every resource while preserving the failed-start error.
        }
    }
  }
}

export interface RemoteAgentHandle<Value = SchemaValue> {
  readonly agentId: string;
  invokeAndAwait(method: string, params: Value, signal?: AbortSignal): Promise<Value | undefined>;
  invokeAndAwaitWithMetadata(
    method: string,
    params: Value,
    signal?: AbortSignal,
  ): Promise<RemoteInvocationResult<Value>>;
  invoke(method: string, params: Value): void;
  invokeWithMetadata(method: string, params: Value): InvocationMetadata;
  schedule(at: Datetime, method: string, params: Value): void;
  scheduleWithMetadata(at: Datetime, method: string, params: Value): ScheduledInvocationReceipt;
  scheduleCancelable(at: Datetime, method: string, params: Value): CancellationToken;
  scheduleCancelableWithMetadata(
    at: Datetime,
    method: string,
    params: Value,
  ): CancelableScheduledInvocationReceipt;
}

export function resolveRemoteAgent(
  agentTypeName: string,
  constructorValue: SchemaValue,
  phantomId?: Uuid,
  configEntries: readonly AgentConfigEntry[] = [],
  mode: 'durable' | 'ephemeral' = 'durable',
): RemoteAgentHandle {
  return resolveRemoteAgentWith(
    agentTypeName,
    constructorValue,
    phantomId,
    configEntries,
    mode,
    (typeName, constructorTree, phantom, config) =>
      new WasmRpc(typeName, constructorTree, phantom, config),
  );
}

export function resolveRemoteAgentFallibly(
  agentTypeName: string,
  constructorValue: SchemaValue,
  phantomId?: Uuid,
  configEntries: readonly AgentConfigEntry[] = [],
  mode: 'durable' | 'ephemeral' = 'durable',
): RemoteAgentHandle {
  return resolveRemoteAgentWith(
    agentTypeName,
    constructorValue,
    phantomId,
    configEntries,
    mode,
    (typeName, constructorTree, phantom, config, agentId) =>
      mapRpcError(`Failed to create remote agent client for ${agentId}`, () =>
        WasmRpc.create(typeName, constructorTree, phantom, config),
      ),
  );
}

function resolveRemoteAgentWith(
  agentTypeName: string,
  constructorValue: SchemaValue,
  phantomId: Uuid | undefined,
  configEntries: readonly AgentConfigEntry[],
  mode: 'durable' | 'ephemeral',
  create: RemoteAgentCreation,
): RemoteAgentHandle {
  const constructorTree = schemaValueToWit(constructorValue);
  const agentId =
    mode === 'ephemeral' ? agentTypeName : makeAgentId(agentTypeName, constructorTree, phantomId);
  const rpc = create(
    agentTypeName,
    constructorTree,
    phantomId,
    configEntries.map((entry) => ({
      path: [...entry.path],
      value: typedSchemaValueToWit(entry.value),
    })),
    agentId,
  );
  return remoteTransport(
    rpc,
    agentId,
    schemaValueToWit,
    schemaValueToWitAsync,
    schemaValueFromWit,
    remoteCallError,
  );
}

/** @internal Transport for compiler-emitted concrete codecs. */
export function resolveWireRemoteAgent(
  agentTypeName: string,
  constructorTree: SchemaValueTree,
  phantomId: Uuid | undefined,
  config: ConstructorParameters<typeof WasmRpc>[3],
  mode: 'durable' | 'ephemeral',
): RemoteAgentHandle<SchemaValueTree> {
  const agentId =
    mode === 'ephemeral' ? agentTypeName : makeAgentId(agentTypeName, constructorTree, phantomId);
  return remoteTransport(
    new WasmRpc(agentTypeName, constructorTree, phantomId, config),
    agentId,
    (value) => value,
    async (value) => value,
    (value) => value,
    (context, error) =>
      new RemoteCallError(
        context,
        mapRemoteCallErrorCause(error, (value) => value),
      ),
  );
}

function remoteTransport<Value>(
  rpc: WasmRpc,
  agentId: string,
  encode: (value: Value) => SchemaValueTree,
  encodeAsync: (value: Value) => Promise<SchemaValueTree>,
  decode: (value: SchemaValueTree) => Value,
  failure: (context: string, error: RpcError) => Error,
): RemoteAgentHandle<Value> {
  const mapRpcError = <T>(context: string, operation: () => T): T => {
    try {
      return operation();
    } catch (error) {
      if (!isRpcError(error)) throw error;
      throw failure(context, error);
    }
  };
  const awaitInvocation = async (
    method: string,
    params: Value,
    signal?: AbortSignal,
  ): Promise<RemoteInvocationResult<Value>> => {
    throwIfAborted(signal);
    const input = await encodeAsync(params);
    let invocation;
    try {
      throwIfAborted(signal);
      invocation = rpc.asyncInvokeAndAwait(method, input, undefined);
    } catch (error) {
      disposeOwnedWitResources(input);
      throw error;
    }
    const future = invocation.future;
    let result;
    try {
      result = await awaitAbortable(future.get(), signal, () => future.cancel());
    } catch (error) {
      if (!isRpcError(error)) throw error;
      throw failure(`Remote agent ${agentId}.${method} errored`, error);
    }
    try {
      return {
        metadata: invocation.metadata,
        value: result === undefined ? undefined : decode(result),
      };
    } catch (error) {
      throw new RemoteOutputError(
        `Remote agent ${agentId}.${method} returned an invalid schema value: ${error instanceof Error ? error.message : String(error)}`,
        { cause: error },
      );
    }
  };
  return {
    agentId,
    invokeAndAwait: async (method, params, signal) =>
      (await awaitInvocation(method, params, signal)).value,
    invokeAndAwaitWithMetadata: awaitInvocation,
    invoke(method, params) {
      mapRpcError(`Remote agent ${agentId}.${method} errored`, () =>
        rpc.invoke(method, encode(params), undefined),
      );
    },
    invokeWithMetadata(method, params) {
      return mapRpcError(`Remote agent ${agentId}.${method} errored`, () =>
        rpc.invoke(method, encode(params), undefined),
      );
    },
    schedule(at, method, params) {
      mapRpcError(`Scheduling remote agent ${agentId}.${method} failed`, () =>
        rpc.scheduleInvocation(at, method, encode(params), undefined),
      );
    },
    scheduleWithMetadata(at, method, params) {
      return mapRpcError(`Scheduling remote agent ${agentId}.${method} failed`, () =>
        rpc.scheduleInvocation(at, method, encode(params), undefined),
      );
    },
    scheduleCancelable(at, method, params) {
      return mapRpcError(`Scheduling remote agent ${agentId}.${method} failed`, () =>
        rpc.scheduleCancelableInvocation(at, method, encode(params), undefined),
      ).cancellationToken;
    },
    scheduleCancelableWithMetadata(at, method, params) {
      return mapRpcError(`Scheduling remote agent ${agentId}.${method} failed`, () =>
        rpc.scheduleCancelableInvocation(at, method, encode(params), undefined),
      );
    },
  };
}
