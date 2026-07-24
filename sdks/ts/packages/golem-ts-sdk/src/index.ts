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

import { ResolvedAgent } from './internal/resolvedAgent';
import { AgentType, Principal } from 'golem:agent/common@2.0.0';
import { SchemaValueTree, uuidToString, parseUuid } from 'golem:core/types@2.0.0';
import type { Snapshot } from 'golem:api/host@1.5.0';
import type { InvocationResult, Tool, ToolError, TypedSchemaValue } from 'golem:tool/common@0.1.0';
import { schemaValueConforms, type ExtendedCommandBody } from './internal/tool';
import {
  deepEqual,
  schemaValueFromWit,
  t,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  v,
} from './internal/schema-model';
import { createCustomError, isAgentError } from './internal/agentError';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from './host/hostapi';
import { AgentInitiator } from './internal/agentInitiator';
import { setAgentId } from './internal/registry/agentId';
import { encodeMultipart, decodeMultipart } from './internal/multipart';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { ToolRegistry } from './internal/registry/toolRegistry';
import { sdkPrincipalFromHost } from './principal';
import type { FluentCodec } from './fluent/schema/codec';
import { awaitAbortable, throwIfAborted } from './internal/pollableUtils';

export { Uuid } from './uuid';
export { ComponentId, AccountId, EnvironmentId } from './ids';
export { ParsedAgentId } from './agentId';
export * from './agentClassName';
export * from './newTypes/textInput';
export * from './newTypes/binaryInput';
export * from './newTypes/multimodalAdvanced';
export { Principal } from './principal';
export { AgentClassName } from './agentClassName';
export { CancellationToken } from 'golem:agent/host@2.0.0';
export { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
export * from './webhook';
export * from './host/hostapi';
export * as oplog from './host/oplog';
export * from './host/guard';
export * from './host/quota';
export * from './host/retry';
export * from './host/result';
export * from './host/saga';
export * from './host/checkpoint';
export * from './host/durable';

// The TypeScript agent authoring surface: `defineAgent` / `method`, the schema
// markers `s`, `clientFor`, the typed host surfaces (keyvalue / blobstore /
// websocket / rdbms), and the `http` helpers. Built on Standard Schema and
// exported from the main entry so it is baked into the bundle injected into
// `agent_guest.wasm` (sharing the runtime registries).
export * from './fluent';

let resolvedAgent: ResolvedAgent | undefined = undefined;
let initializationPrincipal: Principal | undefined = undefined;

interface GolemAgentGuest {
  initialize(agentTypeName: string, input: SchemaValueTree, principal: Principal): Promise<void>;
  discoverAgentTypes(): AgentType[];
  invoke(
    methodName: string,
    input: SchemaValueTree,
    principal: Principal,
  ): Promise<SchemaValueTree | undefined>;
  getDefinition(): AgentType;
}

interface GolemToolGuest {
  discoverTools(): Tool[];
  getTool(name: string): Tool;
  invoke(
    toolName: string,
    commandPath: string[],
    input: TypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
    principal: Principal,
  ): Promise<InvocationResult>;
}

interface SaveSnapshotGuest {
  save(): Promise<Snapshot>;
}

interface LoadSnapshotGuest {
  load(snapshot: Snapshot): Promise<void>;
}

async function initialize(
  agentTypeName: string,
  input: SchemaValueTree,
  principal: Principal,
): Promise<void> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input differs in a re-initialization, then that shouldn't be routed
  // to this already-initialized container either.
  if (resolvedAgent) {
    throw createCustomError(`Agent is already initialized in this container`);
  }

  const registrationError = AgentTypeRegistry.getRegistrationError(agentTypeName);
  if (registrationError) {
    throw createCustomError(formatAgentRegistrationError(agentTypeName, registrationError));
  }

  const initiator: AgentInitiator | undefined = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw createCustomError(
      `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`,
    );
  }

  setAgentId(getRawSelfAgentId());

  const initiateResult = await (initiator.initiateFromWit
    ? initiator.initiateFromWit(input, principal)
    : initiator.initiate(schemaValueFromWit(input), principal));

  if (initiateResult.tag === 'ok') {
    resolvedAgent = initiateResult.val;
    initializationPrincipal = principal;
  } else {
    throw initiateResult.val;
  }
}

async function invokeAgent(
  methodName: string,
  input: SchemaValueTree,
  principal: Principal,
): Promise<SchemaValueTree | undefined> {
  if (!resolvedAgent) {
    throw createCustomError(`Failed to invoke method ${methodName}: agent is not initialized`);
  }

  const result = await resolvedAgent.invoke(methodName, input, principal);

  if (result.tag === 'ok') {
    return result.val;
  } else {
    throw result.val;
  }
}

function discoverTools(): Tool[] {
  const registrationErrors = ToolRegistry.getRegistrationErrors();
  if (registrationErrors.length > 0) {
    throw invalidToolResult(
      `Tool registration failed:\n${registrationErrors
        .map(({ toolName, messages }) => `- Tool "${toolName}": ${messages.join('; ')}`)
        .join('\n')}`,
    );
  }
  return ToolRegistry.getRegisteredTools();
}

function getTool(name: string): Tool {
  const registered = ToolRegistry.getTool(name);
  if (!registered) throw invalidToolName(name);
  return registered;
}

async function invokeTool(
  toolName: string,
  commandPath: string[],
  input: TypedSchemaValue,
  stdin: AsyncIterable<number> | undefined,
  principal: Principal,
): Promise<InvocationResult> {
  let inputAdapter: ToolInputStreamAdapter | undefined;
  let outputAdapter: ToolOutputStreamAdapter | undefined;
  let inputCleanup: Promise<void> | undefined;
  const disposeInput = async (reason?: unknown): Promise<void> => {
    if (!inputCleanup) {
      inputCleanup = inputAdapter ? inputAdapter.dispose(reason) : closeAsyncIterable(stdin);
    }
    await inputCleanup;
  };

  try {
    const resolved = ToolRegistry.resolveInvocation(toolName, commandPath);

    let decodedInput;
    try {
      decodedInput = typedSchemaValueFromWit(input);
    } catch (error) {
      throw invalidToolInput(`malformed invocation input: ${errorMessage(error)}`);
    }

    const prepared = resolved.prepare(decodedInput);
    const body = resolved.command.body;
    if (!body) throw { tag: 'invalid-command-path', val: [...commandPath] } satisfies ToolError;

    const context: Record<string, unknown> = {
      principal: sdkPrincipalFromHost(principal),
    };
    if (body.stdin) {
      if (!stdin && body.stdin.required) {
        throw invalidToolInput('tool invocation did not contain declared stdin stream');
      }
      if (stdin) {
        inputAdapter = readableStreamFromInput(stdin);
        context.stdin = inputAdapter.stream;
      }
    }

    if (body.stdout) {
      outputAdapter = createToolOutputStream();
      context.stdout = outputAdapter.stream;
    }

    const outcome = await prepared.invoke(context);
    const stdout = await outputAdapter?.finish();
    const result = projectToolOutcome(body, outcome, stdout);
    await disposeInput();
    return result;
  } catch (error) {
    await Promise.allSettled([outputAdapter?.abort(error), disposeInput(error)]);
    throw error;
  }
}

function projectToolOutcome(
  body: ExtendedCommandBody,
  outcome: unknown,
  stdout: AsyncIterable<number> | undefined,
): InvocationResult {
  if (!isRecord(outcome) || typeof outcome.tag !== 'string') {
    throw invalidToolResult('tool handler returned an invalid outcome');
  }

  if (outcome.tag === 'ok') {
    if (!Object.prototype.hasOwnProperty.call(outcome, 'value')) {
      throw invalidToolResult('tool handler success is missing its value');
    }
    if (!body.result) {
      if (outcome.value !== undefined) {
        throw invalidToolResult('unit tool handler returned a structured result');
      }
      return { result: undefined, stdout };
    }
    return {
      result: encodeToolValue(body.result.codec, outcome.value, 'tool result'),
      stdout,
    };
  }

  if (outcome.tag === 'err') {
    if (typeof outcome.name !== 'string' || typeof outcome.hasPayload !== 'boolean') {
      throw invalidToolResult('tool handler returned an invalid declared error');
    }
    const errorCase = body.errors.find((candidate) => candidate.name === outcome.name);
    if (!errorCase) {
      throw invalidToolResult(`tool handler returned undeclared error "${outcome.name}"`);
    }

    let payload: TypedSchemaValue;
    if (errorCase.payloadCodec) {
      if (!outcome.hasPayload || !Object.prototype.hasOwnProperty.call(outcome, 'payload')) {
        throw invalidToolResult(`tool error "${outcome.name}" requires a payload`);
      }
      payload = encodeToolValue(
        errorCase.payloadCodec,
        outcome.payload,
        `tool error "${outcome.name}" payload`,
      );
    } else {
      if (outcome.hasPayload || Object.prototype.hasOwnProperty.call(outcome, 'payload')) {
        throw invalidToolResult(`tool error "${outcome.name}" does not declare a payload`);
      }
      payload = typedSchemaValueToWit({
        graph: { defs: new Map(), root: t.tuple([]) },
        value: v.tuple([]),
      });
    }
    throw { tag: 'custom-error', val: payload } satisfies ToolError;
  }

  throw invalidToolResult(`tool handler returned unknown outcome tag "${outcome.tag}"`);
}

function encodeToolValue(codec: FluentCodec, value: unknown, position: string): TypedSchemaValue {
  try {
    const encoded = codec.toValue(value);
    if (!schemaValueConforms(codec.graph, codec.graph.root, encoded)) {
      throw new Error('does not match its declared schema');
    }
    if (!deepEqual(codec.fromValue(encoded), value)) {
      throw new Error('is not canonical for its declared schema');
    }
    return typedSchemaValueToWit({ graph: codec.graph, value: encoded });
  } catch (error) {
    throw invalidToolResult(`${position}: ${errorMessage(error)}`);
  }
}

interface ToolInputStreamAdapter {
  readonly stream: ReadableStream<Uint8Array>;
  dispose(reason?: unknown): Promise<void>;
}

interface ToolOutputStreamAdapter {
  readonly stream: WritableStream<Uint8Array>;
  finish(): Promise<AsyncIterable<number>>;
  abort(reason?: unknown): Promise<void>;
}

function readableStreamFromInput(input: AsyncIterable<number>): ToolInputStreamAdapter {
  const iterator = input[Symbol.asyncIterator]();
  const cancellation = new AbortController();
  let activePull: Promise<void> | undefined;
  let disposal: Promise<void> | undefined;
  let iteratorDisposal: Promise<void> | undefined;

  const disposeIterator = (): Promise<void> => {
    if (!iteratorDisposal) {
      iteratorDisposal = Promise.resolve(iterator.return?.()).then(
        () => undefined,
        () => undefined,
      );
    }
    return iteratorDisposal;
  };

  const dispose = async (reason?: unknown): Promise<void> => {
    if (!disposal) {
      cancellation.abort(reason);
      const pull = activePull;
      disposal = (async () => {
        void disposeIterator();
        if (pull) {
          try {
            await pull;
          } catch {
            // Cancellation only needs to release the input iterator.
          }
        }
        await disposeIterator();
      })();
    }
    await disposal;
  };

  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      const operation = pullInput(iterator, controller, cancellation.signal, disposeIterator);
      const tracked = operation.finally(() => {
        if (activePull === tracked) activePull = undefined;
      });
      activePull = tracked;
      return tracked;
    },
    cancel(reason) {
      return dispose(reason);
    },
  });

  return { stream, dispose };
}

async function pullInput(
  iterator: AsyncIterator<number>,
  controller: ReadableStreamDefaultController<Uint8Array>,
  signal: AbortSignal,
  disposeIterator: () => Promise<void>,
): Promise<void> {
  try {
    throwIfAborted(signal);
    const next = await awaitAbortable(
      Promise.resolve().then(() => iterator.next()),
      signal,
      () => void disposeIterator(),
    );
    if (next.done) {
      closeReadableStream(controller);
      return;
    }

    if (!Number.isInteger(next.value) || next.value < 0 || next.value > 255) {
      throw new TypeError('tool stdin yielded a value outside the byte range');
    }
    controller.enqueue(Uint8Array.of(next.value));
  } catch (error) {
    if (signal.aborted) closeReadableStream(controller);
    else controller.error(error);
  }
}

function createToolOutputStream(): ToolOutputStreamAdapter {
  const chunks: Uint8Array[] = [];
  const invocationCompleted = new Error('tool invocation completed');
  let activeOperation: Promise<void> | undefined;
  let controller: WritableStreamDefaultController | undefined;
  let acceptingOperations = true;
  let failed = false;
  let failure: unknown;

  const recordFailure = (error: unknown): void => {
    if (failed) return;
    failed = true;
    failure = error;
  };

  const track = (operation: Promise<void>): Promise<void> => {
    const tracked = operation
      .catch((error) => {
        recordFailure(error);
        throw error;
      })
      .finally(() => {
        if (activeOperation === tracked) activeOperation = undefined;
      });
    activeOperation = tracked;
    return tracked;
  };

  const settle = async (): Promise<void> => {
    while (true) {
      const operation = activeOperation;
      if (operation) {
        await operation;
        continue;
      }

      // WritableStream starts the next queued sink operation in a promise
      // reaction after the previous operation settles.
      await Promise.resolve();
      if (!activeOperation) return;
    }
  };

  const abort = async (reason?: unknown): Promise<void> => {
    const abortReason = reason === undefined ? new Error('tool stdout stream was aborted') : reason;
    recordFailure(abortReason);
    acceptingOperations = false;
    controller?.error(abortReason);
    try {
      await settle();
    } catch {
      // The invocation path propagates the handler or stream failure that caused the abort.
    }
  };

  const stream = new WritableStream<Uint8Array>({
    start(value) {
      controller = value;
    },
    write(contents) {
      if (!acceptingOperations) return Promise.reject(failed ? failure : invocationCompleted);
      return track(
        Promise.resolve().then(() => {
          if (!(contents instanceof Uint8Array)) {
            throw new TypeError('tool stdout accepts only Uint8Array chunks');
          }
          chunks.push(contents.slice());
        }),
      );
    },
    close() {
      if (!acceptingOperations) return Promise.reject(failed ? failure : invocationCompleted);
      return track(Promise.resolve());
    },
    abort,
  });

  return {
    stream,
    async finish() {
      await settle();
      if (failed) throw failure;
      acceptingOperations = false;
      await settle();
      if (failed) throw failure;
      controller?.error(invocationCompleted);
      return bytesFromChunks(chunks);
    },
    abort,
  };
}

async function* bytesFromChunks(chunks: readonly Uint8Array[]): AsyncIterable<number> {
  for (const chunk of chunks) {
    for (const byte of chunk) yield byte;
  }
}

async function closeAsyncIterable(input: AsyncIterable<number> | undefined): Promise<void> {
  if (!input) return;
  try {
    await input[Symbol.asyncIterator]().return?.();
  } catch {
    // Input stream cleanup is best-effort.
  }
}

function closeReadableStream(controller: ReadableStreamDefaultController<Uint8Array>): void {
  try {
    controller.close();
  } catch {
    // Cancellation may already have closed the web stream.
  }
}

function invalidToolName(name: string): ToolError {
  return { tag: 'invalid-tool-name', val: name };
}

function invalidToolInput(message: string): ToolError {
  return { tag: 'invalid-input', val: message };
}

function invalidToolResult(message: string): ToolError {
  return { tag: 'invalid-result', val: message };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function discoverAgentTypes(): AgentType[] {
  try {
    const registrationErrors = AgentTypeRegistry.getRegistrationErrors();
    if (registrationErrors.length > 0) {
      // Discovery's WIT result cannot carry valid definitions and diagnostics
      // together, so report all invalid agents in one structured error. Valid
      // agents remain registered and can still be initialized independently.
      throw createCustomError(
        `Agent registration failed:\n${registrationErrors
          .map(({ agentTypeName, messages }) =>
            formatAgentRegistrationError(agentTypeName, messages),
          )
          .join('\n')}`,
      );
    }
    return AgentTypeRegistry.getRegisteredAgents();
  } catch (e) {
    // Have to throw RuntimeError, as the discover-agent-types WIT function returns result<list<agent-type>, RuntimeError>
    if (isAgentError(e)) {
      throw e;
    } else {
      throw createCustomError(String(e));
    }
  }
}

function formatAgentRegistrationError(agentTypeName: string, messages: readonly string[]): string {
  return `- Agent "${agentTypeName}": ${messages.join('; ')}`;
}

function getDefinition(): AgentType {
  if (!resolvedAgent) {
    throw new Error('Failed to get agent definition: agent is not initialized');
  }

  return resolvedAgent.getAgentType();
}

function serializePrincipal(p: Principal): object {
  switch (p.tag) {
    case 'anonymous':
      return { tag: 'anonymous' };
    case 'agent':
      return {
        tag: 'agent',
        val: {
          componentId: uuidToString(p.val.agentId.componentId.uuid),
          agentId: p.val.agentId.agentId,
        },
      };
    case 'golem-user':
      return {
        tag: 'golem-user',
        val: { accountId: uuidToString(p.val.accountId.uuid) },
      };
    case 'oidc':
      return {
        tag: 'oidc',
        val: {
          sub: p.val.sub,
          issuer: p.val.issuer,
          email: p.val.email ?? null,
          name: p.val.name ?? null,
          emailVerified: p.val.emailVerified ?? null,
          givenName: p.val.givenName ?? null,
          familyName: p.val.familyName ?? null,
          picture: p.val.picture ?? null,
          preferredUsername: p.val.preferredUsername ?? null,
          claims: p.val.claims,
        },
      };
  }
}

function deserializePrincipal(obj: any): Principal {
  switch (obj.tag) {
    case 'anonymous':
      return { tag: 'anonymous' };
    case 'agent':
      return {
        tag: 'agent',
        val: {
          agentId: {
            componentId: { uuid: parseUuid(obj.val.componentId) },
            agentId: obj.val.agentId,
          },
        },
      };
    case 'golem-user':
      return {
        tag: 'golem-user',
        val: { accountId: { uuid: parseUuid(obj.val.accountId) } },
      };
    case 'oidc': {
      if (
        !obj.val ||
        typeof obj.val.sub !== 'string' ||
        typeof obj.val.issuer !== 'string' ||
        typeof obj.val.claims !== 'string'
      ) {
        throw new Error('Missing required fields (sub, issuer, claims) in oidc principal');
      }
      return {
        tag: 'oidc',
        val: {
          sub: obj.val.sub,
          issuer: obj.val.issuer,
          email: obj.val.email ?? undefined,
          name: obj.val.name ?? undefined,
          emailVerified: obj.val.emailVerified ?? undefined,
          givenName: obj.val.givenName ?? undefined,
          familyName: obj.val.familyName ?? undefined,
          picture: obj.val.picture ?? undefined,
          preferredUsername: obj.val.preferredUsername ?? undefined,
          claims: obj.val.claims,
        },
      };
    }
    default:
      throw new Error(`Unknown principal tag: ${obj.tag}`);
  }
}

async function save(): Promise<{ payload: Uint8Array; mimeType: string }> {
  if (!resolvedAgent) {
    throw new Error('Failed to save agent snapshot: agent is not initialized');
  }

  const { data: agentSnapshot, mimeType } = await resolvedAgent.saveSnapshot();
  const principal = initializationPrincipal ?? { tag: 'anonymous' };
  const serializedPrincipal = serializePrincipal(principal);

  if (mimeType.startsWith('multipart/mixed')) {
    // Multipart snapshot: the state JSON part already contains agent properties.
    // We need to inject version and principal into the state part.
    const boundaryMatch = mimeType.match(/boundary=([^\s;]+)/);
    if (!boundaryMatch) {
      throw new Error('multipart/mixed snapshot missing boundary parameter');
    }
    const boundary = boundaryMatch[1];
    const parts = decodeMultipart(agentSnapshot, boundary);

    const stateIdx = parts.findIndex((p) => p.name === 'state');
    if (stateIdx === -1) {
      throw new Error('multipart snapshot missing "state" part');
    }

    const stateJson = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
    const envelope = { version: 1, principal: serializedPrincipal, state: stateJson };
    parts[stateIdx] = {
      ...parts[stateIdx],
      body: new TextEncoder().encode(JSON.stringify(envelope)),
    };

    const { data, boundary: newBoundary } = encodeMultipart(parts);
    return {
      payload: data,
      mimeType: `multipart/mixed; boundary=${newBoundary}`,
    };
  } else if (mimeType === 'application/json') {
    // JSON snapshot: wrap in envelope { version, principal, state }
    const state = JSON.parse(new TextDecoder().decode(agentSnapshot));
    const envelope = { version: 1, principal: serializedPrincipal, state };
    return {
      payload: new TextEncoder().encode(JSON.stringify(envelope)),
      mimeType: 'application/json',
    };
  } else {
    // Binary snapshot: version-2 binary envelope with principal
    const principalJson = JSON.stringify(serializedPrincipal);
    const principalBytes = new TextEncoder().encode(principalJson);

    const totalLength = 1 + 4 + principalBytes.length + agentSnapshot.length;
    const fullSnapshot = new Uint8Array(totalLength);
    const view = new DataView(fullSnapshot.buffer);
    view.setUint8(0, 2); // version
    view.setUint32(1, principalBytes.length, false); // big-endian
    fullSnapshot.set(principalBytes, 5);
    fullSnapshot.set(agentSnapshot, 5 + principalBytes.length);

    return { payload: fullSnapshot, mimeType: 'application/octet-stream' };
  }
}

async function load(snapshot: { payload: Uint8Array; mimeType: string }): Promise<void> {
  const bytes = snapshot.payload;

  if (resolvedAgent) {
    throw `Agent is already initialized in this container`;
  }

  const [agentTypeName, agentParameters] = getRawSelfAgentId().parsed();
  const registrationError = AgentTypeRegistry.getRegistrationError(agentTypeName);
  if (registrationError) {
    // The snapshot WIT interface returns `result<_, string>`, not AgentError.
    throw formatAgentRegistrationError(agentTypeName, registrationError);
  }

  let agentSnapshot: Uint8Array;
  let agentSnapshotMimeType: string | undefined;
  let principal: Principal;

  if (snapshot.mimeType.startsWith('multipart/mixed')) {
    // Multipart snapshot: extract principal from the state JSON part
    const boundaryMatch = snapshot.mimeType.match(/boundary=([^\s;]+)/);
    if (!boundaryMatch) {
      throw 'multipart/mixed snapshot missing boundary parameter';
    }
    const boundary = boundaryMatch[1];
    const parts = decodeMultipart(bytes, boundary);

    const stateIdx = parts.findIndex((p) => p.name === 'state');
    if (stateIdx === -1) {
      throw 'multipart snapshot missing "state" part';
    }

    const envelope = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
    principal = envelope.principal
      ? deserializePrincipal(envelope.principal)
      : (initializationPrincipal ?? { tag: 'anonymous' });

    if (envelope.state === undefined) {
      throw `multipart state part missing 'state' field`;
    }

    // Replace the state part body with just the agent properties (strip version/principal)
    parts[stateIdx] = {
      ...parts[stateIdx],
      body: new TextEncoder().encode(JSON.stringify(envelope.state)),
    };

    // Re-encode the parts for loadSnapshot
    const { data: reencoded, boundary: newBoundary } = encodeMultipart(parts);
    agentSnapshot = reencoded;
    agentSnapshotMimeType = `multipart/mixed; boundary=${newBoundary}`;
  } else if (snapshot.mimeType === 'application/json') {
    // JSON snapshot: unwrap envelope { version, principal, state }
    const envelope = JSON.parse(new TextDecoder().decode(bytes));
    principal = envelope.principal
      ? deserializePrincipal(envelope.principal)
      : (initializationPrincipal ?? { tag: 'anonymous' });
    if (envelope.state === undefined) {
      throw `JSON snapshot missing 'state' field`;
    }
    agentSnapshot = new TextEncoder().encode(JSON.stringify(envelope.state));
    agentSnapshotMimeType = 'application/json';
  } else {
    // Custom binary snapshot with version envelope
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const version = view.getUint8(0);

    if (version === 1) {
      agentSnapshot = bytes.slice(1);
      principal = initializationPrincipal ?? { tag: 'anonymous' };
    } else if (version === 2) {
      const principalLen = view.getUint32(1, false); // big-endian
      const principalBytes = bytes.slice(5, 5 + principalLen);
      principal = deserializePrincipal(JSON.parse(new TextDecoder().decode(principalBytes)));
      agentSnapshot = bytes.slice(5 + principalLen);
    } else {
      throw `Unsupported snapshot version ${version}`;
    }
  }

  initializationPrincipal = principal;

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  const initiateResult = await initiator.initiate(agentParameters, principal);

  if (initiateResult.tag === 'ok') {
    const agent = initiateResult.val;
    await agent.loadSnapshot(agentSnapshot, agentSnapshotMimeType);

    resolvedAgent = agent;
  } else {
    // Throwing a String because the load WIT function returns result<_, string>
    let errorString = 'Failed to construct agent';
    try {
      errorString = JSON.stringify(initiateResult.val);
    } catch (e) {
      console.error('Failed to stringify agent construction error: ', e);
    }
    throw errorString;
  }
}

export const golemAgent200Guest: GolemAgentGuest = {
  initialize,
  discoverAgentTypes,
  invoke: invokeAgent,
  getDefinition,
};

// The current wasm-rquickjs wrapper looks up the guest export by the WIT interface
// short name (`guest.discoverAgentTypes` of golem:agent/guest@2.0.0). Export `guest`
// as an alias of golemAgent200Guest so the generated wrapper finds it.
export const guest: GolemAgentGuest = golemAgent200Guest;

export const golemTool010Guest: GolemToolGuest = {
  discoverTools,
  getTool,
  invoke: invokeTool,
};

// The generated wrapper also looks up the tool guest by its short interface name.
export const tool: GolemToolGuest = golemTool010Guest;

export const saveSnapshot: SaveSnapshotGuest = {
  save,
};

export const loadSnapshot: LoadSnapshotGuest = {
  load,
};
