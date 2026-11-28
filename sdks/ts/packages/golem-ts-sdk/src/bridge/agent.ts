// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { Invocation as HostInvocation, WasmRpc } from 'golem:agent/host@2.0.0';
import * as wasiEnv from 'wasi:cli/environment@0.3.0';
import { withSignal } from 'wasm-rquickjs:import-abort';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import type { Datetime, RpcError } from 'golem:agent/host@2.0.0';
import { throwIfAborted } from '../internal/pollableUtils';
import {
  SchemaEncodeError,
  schemaValueFromWit,
  schemaValueToWit,
  schemaValueToWitAsync,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  preflightWitValueTree,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
  type TypedSchemaValue,
} from '../internal/schema-model';
import { AgentId } from '../agentId';
import { InvocationId, invocationMetadataFromHost, type InvocationMetadata } from '../invocationId';
import type { InputRecord, MethodSpec } from '../method';
import type { SchemaCodec } from '../schema/codec';
import { compileSchema } from '../schema/adapter';
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
  | { readonly tag: 'conflict'; readonly details: string }
  /** The awaited invocation was cancelled before it started. */
  | { readonly tag: 'cancelled'; readonly details: string }
  /** A cancellation found the invocation already running. */
  | { readonly tag: 'cannot-cancel-running'; readonly details: string };

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

/** The lifecycle state of an accepted invocation. */
export type InvocationStatus =
  /** Accepted and waiting for its turn */
  | 'pending'
  /** Accepted and waiting for its scheduled time */
  | 'scheduled'
  /** Started and executing */
  | 'running'
  /** Started and waiting, for example for a promise or a timer */
  | 'waiting'
  /** Started and waiting to retry after a failure */
  | 'retrying'
  /** Started, and its agent was interrupted by an operator */
  | 'interrupted'
  /** Finished with a result */
  | 'completed'
  /** Finished with a failure */
  | 'failed'
  /** Withdrawn before it started */
  | 'cancelled'
  /** Recorded its result, then its agent failed during the invocation's background work */
  | 'background-failed';

/** Options of {@link Invocation.awaitResult}. */
export interface InvocationAwaitOptions {
  /**
   * Stops waiting when the signal aborts: the returned promise rejects with the signal's reason.
   * Aborting only detaches; the invocation keeps running.
   */
  signal?: AbortSignal;
}

type DecodeInvocationOutput<T> = (output: SchemaValueTree | undefined) => T;

let invocationFromHost: <T>(
  raw: HostInvocation,
  decode: DecodeInvocationOutput<T>,
  id?: InvocationId,
) => Invocation<T>;
let mapInvocationOutput: <T, U>(invocation: Invocation<T>, f: (value: T) => U) => Invocation<U>;

/**
 * The handle of an accepted invocation of an agent method, which `trigger` and `schedule`
 * return.
 *
 * The ID only locates the invocation; it is not authority. Only the caller that started the
 * invocation, or a caller with the applicable management permission, can observe or cancel it;
 * any other caller fails with a `denied` {@link RemoteCallError}. Store {@link Invocation.id} to
 * await the invocation from a later invocation of the same agent, and rebuild the handle with
 * {@link Invocation.get}. Dropping the handle, or aborting {@link Invocation.awaitResult}, only
 * detaches: the invocation keeps running.
 *
 * `trigger` and `schedule` reject methods with stream inputs or outputs, and never send a scope
 * card.
 * Cancelling a running invocation is not supported: {@link Invocation.cancel} fails with
 * `cannot-cancel-running`.
 */
export class Invocation<T> {
  /** The ID of the invocation. */
  readonly id: InvocationId;
  readonly #raw: HostInvocation;
  readonly #decode: DecodeInvocationOutput<T>;

  static {
    invocationFromHost = (raw, decode, id) => new Invocation(raw, decode, id);
    mapInvocationOutput = <T, U>(invocation: Invocation<T>, f: (value: T) => U) => {
      const decode = invocation.#decode;
      return new Invocation(invocation.#raw, (output) => f(decode(output)), invocation.id);
    };
  }

  private constructor(raw: HostInvocation, decode: DecodeInvocationOutput<T>, id?: InvocationId) {
    this.#raw = raw;
    this.#decode = decode;
    this.id = id ?? invocationIdFromHost(raw.id());
  }

  /**
   * Rebuilds the handle of the invocation with the given ID. Makes no remote call. `method` is
   * the method spec of the invoked method, whose `returns` schema decodes the result.
   */
  static get<T>(
    id: InvocationId,
    method: Pick<MethodSpec<InputRecord, T>, 'returns'>,
  ): Invocation<T> {
    const output = remoteOutputCodec(compileSchema(method.returns));
    return mapInvocationOutput(
      invocationValueById(id),
      (value) => decodeRemoteOutput(`Invocation ${id.toString()}`, output, value) as T,
    );
  }

  /**
   * The current lifecycle state of the invocation. Rejects with a `not-found`
   * {@link RemoteCallError} when its agent or the invocation no longer exists.
   */
  async status(): Promise<InvocationStatus> {
    try {
      return await this.#raw.status();
    } catch (error) {
      throw invocationFailure(
        `Getting the status of invocation ${this.id.toString()} failed`,
        error,
      );
    }
  }

  /**
   * Awaits the result of the invocation, without starting, resuming or resubmitting it. Rejects
   * with a `cancelled` {@link RemoteCallError} when the invocation was cancelled before it
   * started. Aborting `signal` only detaches: the promise rejects with the signal's reason and the
   * invocation keeps running.
   */
  async awaitResult(options: InvocationAwaitOptions = {}): Promise<T> {
    const { signal } = options;
    throwIfAborted(signal);
    let output;
    try {
      output = await withSignal(signal, () => this.#raw.await_());
    } catch (error) {
      throw invocationFailure(`Awaiting invocation ${this.id.toString()} failed`, error);
    }
    return this.#decode(output);
  }

  /**
   * Withdraws the invocation if it has not started yet, and resolves to `true`. Resolves to
   * `false` when the invocation already finished or was already cancelled, and rejects with a
   * `cannot-cancel-running` {@link RemoteCallError} when it is running.
   */
  async cancel(): Promise<boolean> {
    try {
      return await this.#raw.cancel();
    } catch (error) {
      throw invocationFailure(`Cancelling invocation ${this.id.toString()} failed`, error);
    }
  }
}

/**
 * @internal Rebuilds the handle of the invocation with the given ID, whose result is the schema
 * value the invoked method returned. Makes no remote call.
 */
export function invocationValueById(id: InvocationId): Invocation<SchemaValue | undefined> {
  const context = `Invocation ${id.toString()}`;
  return invocationFromHost(
    HostInvocation.get({ owner: { tag: 'agent', val: AgentId.toWit(id.agentId) }, id: id.uuid }),
    (tree) =>
      tree === undefined ? undefined : decodeRemoteSchemaValue(context, schemaValueFromWit, tree),
    id,
  );
}

/**
 * @internal The error of `trigger` and `schedule` for a method whose input or output contains
 * streams, which only an awaited invocation can carry. `context` names the method, for example
 * `Remote agent Counter.increment`.
 */
export function streamingMethodNotAwaitedError(context: string): SchemaEncodeError {
  return new SchemaEncodeError(`${context}: live streams require invoke-and-await`);
}

/** @internal The handle of the same invocation, with `f` applied to its decoded result. */
export function mapInvocation<T, U>(invocation: Invocation<T>, f: (value: T) => U): Invocation<U> {
  return mapInvocationOutput(invocation, f);
}

function invocationIdFromHost(id: ReturnType<HostInvocation['id']>): InvocationId {
  if (id.owner.tag !== 'agent') {
    throw new RemoteCallError('Invalid invocation handle', {
      tag: 'protocol-error',
      details: `invocation ${id.owner.tag} owner is not an agent`,
    });
  }
  return new InvocationId(AgentId.fromWit(id.owner.val), id.id);
}

function invocationFailure(context: string, error: unknown): unknown {
  return isRpcError(error) ? remoteCallError(context, error) : error;
}

/** @internal The output shape of a remote method. */
export type RemoteOutputCodec = { tag: 'unit' } | { tag: 'single'; codec: SchemaCodec };

/** @internal The output shape of a remote method whose result `codec` decodes. */
export function remoteOutputCodec(codec: SchemaCodec): RemoteOutputCodec {
  return codec.isUnit ? { tag: 'unit' } : { tag: 'single', codec };
}

function decodeRemoteSchemaValue<Value>(
  context: string,
  decode: (value: SchemaValueTree) => Value,
  tree: SchemaValueTree,
): Value {
  try {
    return decode(tree);
  } catch (error) {
    throw new RemoteOutputError(
      `${context} returned an invalid schema value: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    );
  }
}

/**
 * @internal Decodes the result of a remote method. `context` names the method in error
 * messages, for example `Remote agent Counter.increment`.
 */
export function decodeRemoteOutput(
  context: string,
  output: RemoteOutputCodec,
  value: SchemaValue | undefined,
): unknown {
  if (output.tag === 'unit') {
    if (value !== undefined) {
      throw new RemoteOutputError(`${context} returned a value for a unit output`);
    }
    return undefined;
  }
  if (value === undefined) {
    throw new RemoteOutputError(`${context} returned no value for a non-unit output`);
  }
  try {
    assertValueMatchesType(value, output.codec.graph.root, output.codec.graph);
    return output.codec.fromValue(value);
  } catch (error) {
    throw new RemoteOutputError(
      `${context} returned an invalid output: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    );
  }
}

function assertValueMatchesType(value: SchemaValue, type: SchemaType, graph: SchemaGraph): void {
  let body = type.body;
  const seenRefs = new Set<string>();
  while (body.tag === 'ref') {
    if (seenRefs.has(body.id)) throw new Error(`Cyclic schema reference ${body.id}`);
    seenRefs.add(body.id);
    const def = graph.defs.get(body.id);
    if (!def) throw new Error(`Missing schema definition ${body.id}`);
    body = def.body.body;
  }

  if (value.tag !== body.tag) {
    throw new Error(`Expected schema value ${body.tag}, got ${value.tag}`);
  }
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
    case 'cancelled':
    case 'cannot-cancel-running':
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
    case 'cancelled':
    case 'cannot-cancel-running':
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
    case 'cancelled':
    case 'cannot-cancel-running':
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
  /**
   * Triggers `method` without awaiting its result, and resolves to the invocation's handle once
   * the target has durably accepted it. Never sends a scope card; rejects stream inputs.
   */
  trigger(method: string, params: Value): Promise<Invocation<Value | undefined>>;
  /**
   * Schedules `method` for `at`, and resolves to the invocation's handle once the target has
   * durably accepted it. Never sends a scope card; rejects stream inputs.
   */
  schedule(at: Datetime, method: string, params: Value): Promise<Invocation<Value | undefined>>;
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
  const agentId = AgentId.fromWit(created.agentId);
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
    WasmRpc.getById(agentTypeName, AgentId.toWit(agentId)),
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
    const agentId = AgentId.fromWit(rawAgentId);
    return boundRemoteAgent(`${expectedTypeName}[${agentId.toString()}]`, rpc);
  } catch (error) {
    (rpc as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.();
    throw error;
  }
}

function boundRemoteAgent(target: string, rpc: WasmRpc): RemoteAgentHandle {
  const agentId = () =>
    mapAsyncRpcError(`Failed to get the ID of remote agent ${target}`, async () =>
      AgentId.fromWit(await rpc.agentId()),
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
  const mapFailure = (context: string, error: unknown): unknown =>
    isRpcError(error) ? failure(context, error) : error;
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
    const context = `Remote agent ${target}.${method}`;
    let completed;
    try {
      // An abort after the call started drops the call, which withdraws the invocation on a
      // best-effort basis while it has not started.
      completed = await withSignal(signal, () => rpc.invokeAndAwait(method, input, scope));
    } catch (error) {
      disposeOwnedWitResources(input);
      throw mapFailure(`${context} errored`, error);
    }
    const metadata = invocationMetadataFromHost(completed.metadata);
    const result = completed.result;
    return {
      metadata,
      value: result === undefined ? undefined : decodeRemoteSchemaValue(context, decode, result),
    };
  };
  const start = async (
    method: string,
    params: Value,
    call: (input: SchemaValueTree) => Promise<HostInvocation>,
  ): Promise<Invocation<Value | undefined>> => {
    const context = `Remote agent ${target}.${method}`;
    const input = encode(params);
    let raw;
    try {
      raw = await call(input);
    } catch (error) {
      disposeOwnedWitResources(input);
      throw mapFailure(`${context} errored`, error);
    }
    return invocationFromHost(raw, (output) =>
      output === undefined ? undefined : decodeRemoteSchemaValue(context, decode, output),
    );
  };
  return {
    target,
    agentId,
    invokeAndAwait: async (method, params, signal, scopeCard) =>
      (await awaitInvocation(method, params, signal, scopeCard)).value,
    invokeAndAwaitWithMetadata: awaitInvocation,
    trigger: (method, params) => start(method, params, (input) => rpc.trigger(method, input)),
    schedule: (at, method, params) =>
      start(method, params, (input) => rpc.schedule(at, method, input)),
  };
}
