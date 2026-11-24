// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { WasmRpc } from 'golem:agent/host@2.0.0';
import * as wasiEnv from 'wasi:cli/environment@0.3.0';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import type { CancellationToken, Datetime, RpcError } from 'golem:agent/host@2.0.0';
import { awaitAbortable, throwIfAborted } from '../internal/pollableUtils';
import {
  schemaValueFromWit,
  schemaValueToWit,
  schemaValueToWitAsync,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  preflightWitValueTree,
  type SchemaValue,
  type TypedSchemaValue,
} from '../internal/schema-model';
import { AgentId } from '../agentId';
import {
  invocationMetadataFromHost,
  type CancelableScheduledInvocationReceipt,
  type InvocationMetadata,
  type ScheduledInvocationReceipt,
} from '../invocationId';
import { borrowScopeCardInternal, type ScopeCard } from '../host/permissions';
import { PERMISSION_CARD_INTERNAL } from '../internal/schema-model/permissionCardInternal';
import { restoreGuestPermissionCardHandleFromWire } from '../internal/schema-model/permissionCardHandle';

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
  | { readonly tag: 'remote-agent-error'; readonly error: RemoteAgentError<Value> }
  /**
   * A keyed create found an agent that already has the key, or the agent at the ID that a key
   * derives has another key.
   */
  | { readonly tag: 'conflict'; readonly details: string };

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

function isRpcError(error: unknown): error is RpcError {
  if (error === null || typeof error !== 'object') return false;

  switch ((error as { tag?: unknown }).tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
    case 'remote-agent-error':
    case 'conflict':
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
    case 'conflict':
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
    case 'conflict':
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

async function mapAsyncRpcError<T>(context: string, operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
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

/**
 * Give the permission cards of an input tree that never reached the host back
 * to their cards, and release every other resource the tree still owns.
 */
function returnUnsentWitResources(tree: SchemaValueTree): void {
  for (const node of tree.valueNodes) {
    if (
      node.tag === 'permission-card-handle' &&
      node.val !== undefined &&
      restoreGuestPermissionCardHandleFromWire(PERMISSION_CARD_INTERNAL, node.val, node)
    ) {
      (node as { val: unknown }).val = undefined;
    }
  }
  disposeOwnedWitResources(tree);
}

export interface RemoteAgentHandle<Value = SchemaValue> {
  /** Names the target in error messages: the agent type, and the agent ID when it is known. */
  readonly target: string;
  /**
   * The ID of the agent this handle connects to. On a handle of {@link upsertRemoteAgent} it
   * creates the agent when it is missing, without invoking a method.
   */
  agentId(): Promise<AgentId>;
  /**
   * Invoke `method` and await its result. A `scopeCard` is borrowed for the
   * call: the callee holds its authority until the invocation ends, and the
   * caller keeps the card.
   */
  invokeAndAwait(
    method: string,
    params: Value,
    signal?: AbortSignal,
    scopeCard?: ScopeCard,
  ): Promise<Value | undefined>;
  invokeAndAwaitWithMetadata(
    method: string,
    params: Value,
    signal?: AbortSignal,
    scopeCard?: ScopeCard,
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

const wireRemotes = new WeakMap<RemoteAgentHandle, RemoteAgentHandle<SchemaValueTree>>();

/** Internal companion transport for prepared, capability-free invocation codecs. */
export function wireRemoteAgent(
  remote: RemoteAgentHandle,
): RemoteAgentHandle<SchemaValueTree> | undefined {
  return wireRemotes.get(remote);
}

/** Options of {@link createRemoteAgent}. */
export interface RemoteAgentCreateOptions {
  /**
   * Creates the keyed agent of the constructor parameters, and fails with a `conflict`
   * {@link RemoteCallError} when it already exists. Otherwise the new agent has no key and a new
   * ID.
   */
  readonly keyed?: boolean;
  /** Overrides of the agent type's local configuration values. */
  readonly config?: readonly AgentConfigEntry[];
  /** Environment variables added to the ones the manifest defines. */
  readonly env?: readonly (readonly [string, string])[];
}

function configEntriesToWit(entries: readonly AgentConfigEntry[]) {
  return entries.map((entry) => ({
    path: [...entry.path],
    value: typedSchemaValueToWit(entry.value),
  }));
}

/**
 * Connects to the keyed agent of the given type and constructor parameters. A durable target is
 * created when it is missing; for an ephemeral target, each invocation runs on a new one-shot
 * instance. Host failures throw a {@link RemoteCallError}.
 */
export function upsertRemoteAgent(
  agentTypeName: string,
  constructorValue: SchemaValue,
): RemoteAgentHandle {
  const rpc = mapRpcError(`Failed to upsert remote agent ${agentTypeName}`, () =>
    WasmRpc.upsert(agentTypeName, schemaValueToWit(constructorValue)),
  );
  return boundRemoteAgent(agentTypeName, rpc);
}

/**
 * Creates a durable agent of the given type, and connects to it after its initialization. A
 * keyed create that finds the key fails with a `conflict` {@link RemoteCallError}.
 */
export async function createRemoteAgent(
  agentTypeName: string,
  constructorValue: SchemaValue,
  options: RemoteAgentCreateOptions = {},
): Promise<RemoteAgentHandle> {
  const created = await mapAsyncRpcError(`Failed to create remote agent ${agentTypeName}`, () =>
    WasmRpc.create(agentTypeName, schemaValueToWit(constructorValue), {
      keyed: options.keyed ?? false,
      agentConfig: configEntriesToWit(options.config ?? []),
      env: (options.env ?? []).map(([name, value]) => [name, value] as [string, string]),
    }),
  );
  const agentId = AgentId.from(created.agentId);
  return boundRemoteAgent(`${agentTypeName}[${agentId.toString()}]`, created.client);
}

/**
 * Connects to the existing agent of the given type with the given ID. Never creates the agent: an
 * unknown ID, or the ID of an agent of another type, fails with a `not-found`
 * {@link RemoteCallError}.
 */
export function getRemoteAgentById(agentTypeName: string, agentId: AgentId): RemoteAgentHandle {
  const target = `${agentTypeName}[${agentId.toString()}]`;
  const rpc = mapRpcError(`Failed to connect to remote agent ${target}`, () =>
    WasmRpc.getById(agentTypeName, agentId),
  );
  return boundRemoteAgent(target, rpc);
}

/** @internal The agent type of the current agent, which Golem sets in `GOLEM_AGENT_TYPE`. */
export function selfAgentTypeName(): string | undefined {
  return wasiEnv.getEnvironment().find(([key]) => key === 'GOLEM_AGENT_TYPE')?.[1];
}

/** Bind self without creating or initializing a target, retaining its recorded identity. */
export function bindSelfRemoteAgent(expectedTypeName: string): RemoteAgentHandle {
  const [rawAgentId, rpc] = mapRpcError('Failed to bind self', () => WasmRpc.bindSelf());
  try {
    const typeName = selfAgentTypeName();
    if (typeName !== expectedTypeName) {
      throw remoteCallError('Failed to bind self', {
        tag: 'remote-agent-error',
        val: {
          tag: 'invalid-type',
          val: `Full agent client '${expectedTypeName}' cannot bind agent type '${typeName ?? '<unknown>'}'`,
        },
      });
    }
    const agentId = AgentId.from(rawAgentId);
    return boundRemoteAgent(`${expectedTypeName}[${agentId.toString()}]`, rpc);
  } catch (error) {
    (rpc as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.();
    throw error;
  }
}

function boundRemoteAgent(target: string, rpc: WasmRpc): RemoteAgentHandle {
  const agentId = () =>
    mapAsyncRpcError(`Failed to get the ID of remote agent ${target}`, async () =>
      AgentId.from(await rpc.agentId()),
    );
  const remote = remoteTransport(
    rpc,
    target,
    agentId,
    schemaValueToWit,
    schemaValueToWitAsync,
    schemaValueFromWit,
    remoteCallError,
  );
  wireRemotes.set(
    remote,
    remoteTransport(
      rpc,
      target,
      agentId,
      (value) => value,
      async (value) => value,
      (value) => {
        try {
          preflightWitValueTree(value.valueNodes, value.root);
        } catch (error) {
          // Preserve the ordinary failed-preflight resource cleanup.
          schemaValueFromWit(value);
          throw error;
        }
        return value;
      },
      remoteCallError,
    ),
  );
  return remote;
}

function remoteTransport<Value>(
  rpc: WasmRpc,
  target: string,
  agentId: () => Promise<AgentId>,
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
    scopeCard?: ScopeCard,
  ): Promise<RemoteInvocationResult<Value>> => {
    throwIfAborted(signal);
    // Check the scope card before the input moves any argument capability.
    if (scopeCard !== undefined) borrowScopeCardInternal(PERMISSION_CARD_INTERNAL, scopeCard);
    const input = await encodeAsync(params);
    let scope;
    try {
      throwIfAborted(signal);
      scope =
        scopeCard === undefined
          ? undefined
          : borrowScopeCardInternal(PERMISSION_CARD_INTERNAL, scopeCard);
    } catch (error) {
      returnUnsentWitResources(input);
      throw error;
    }
    let invocation;
    try {
      invocation = rpc.asyncInvokeAndAwait(method, input, scope);
    } catch (error) {
      disposeOwnedWitResources(input);
      throw error;
    }
    // Absent metadata means the host rejected the call; the future reports the error.
    const metadata = invocation.metadata && invocationMetadataFromHost(invocation.metadata);
    const future = invocation.future;
    let result;
    try {
      result = await awaitAbortable(future.get(), signal, () => future.cancel());
    } catch (error) {
      if (!isRpcError(error)) throw error;
      throw failure(`Remote agent ${target}.${method} errored`, error);
    }
    if (metadata === undefined) {
      if (result !== undefined) disposeOwnedWitResources(result);
      throw new RemoteCallError(`Remote agent ${target}.${method} errored`, {
        tag: 'protocol-error',
        details: 'completed invocation has no invocation metadata',
      });
    }
    try {
      return {
        metadata,
        value: result === undefined ? undefined : decode(result),
      };
    } catch (error) {
      throw new RemoteOutputError(
        `Remote agent ${target}.${method} returned an invalid schema value: ${error instanceof Error ? error.message : String(error)}`,
        { cause: error },
      );
    }
  };
  return {
    target,
    agentId,
    invokeAndAwait: async (method, params, signal, scopeCard) =>
      (await awaitInvocation(method, params, signal, scopeCard)).value,
    invokeAndAwaitWithMetadata: awaitInvocation,
    invoke(method, params) {
      mapRpcError(`Remote agent ${target}.${method} errored`, () =>
        rpc.invoke(method, encode(params), undefined),
      );
    },
    invokeWithMetadata(method, params) {
      const metadata = mapRpcError(`Remote agent ${target}.${method} errored`, () =>
        rpc.invoke(method, encode(params), undefined),
      );
      return invocationMetadataFromHost(metadata);
    },
    schedule(at, method, params) {
      mapRpcError(`Scheduling remote agent ${target}.${method} failed`, () =>
        rpc.scheduleInvocation(at, method, encode(params), undefined),
      );
    },
    scheduleWithMetadata(at, method, params) {
      const receipt = mapRpcError(`Scheduling remote agent ${target}.${method} failed`, () =>
        rpc.scheduleInvocation(at, method, encode(params), undefined),
      );
      return Object.freeze({ metadata: invocationMetadataFromHost(receipt.metadata) });
    },
    scheduleCancelable(at, method, params) {
      return mapRpcError(`Scheduling remote agent ${target}.${method} failed`, () =>
        rpc.scheduleCancelableInvocation(at, method, encode(params), undefined),
      ).cancellationToken;
    },
    scheduleCancelableWithMetadata(at, method, params) {
      const receipt = mapRpcError(`Scheduling remote agent ${target}.${method} failed`, () =>
        rpc.scheduleCancelableInvocation(at, method, encode(params), undefined),
      );
      return Object.freeze({
        metadata: invocationMetadataFromHost(receipt.metadata),
        cancellationToken: receipt.cancellationToken,
      });
    },
  };
}
