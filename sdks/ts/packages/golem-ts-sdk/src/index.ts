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
import type { ByteStreamItem, ToolOutputWriter } from 'golem:tool/streams@0.1.0';
import type { ExtendedCommandBody } from './internal/tool';
import { createCustomError, isAgentError } from './internal/agentError';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from './host/hostapi';
import { AgentInitiator } from './internal/agentInitiator';
import { encodeMultipart, decodeMultipart, extractBoundary } from './internal/multipart';
import { normalizeContentType, SnapshotError, validatePartName } from './snapshot';
import type { SnapshotPart } from './snapshot';
import { decodeSnapshotDatabases, SnapshotDatabases } from './internal/databaseSnapshot';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { ToolRegistry } from './internal/registry/toolRegistry';
import { sdkPrincipalFromHost } from './principal';
import {
  encodeDeclaredToolErrorPayload,
  encodeToolValueAsync,
  invalidToolResult,
  isDeclaredToolError,
} from './internal/tool/invocationResult';
import { closeAsyncIterable } from './internal/tool/asyncIterable';
import { awaitAbortable, throwIfAborted } from './internal/pollableUtils';
import { ToolStreamError, toolStreamFailureFromError } from './internal/tool/startedToolInvocation';
import './schema/zod';
import './schema/valibot';
import './schema/arktype';
import './schema/effect';

export { Uuid } from './uuid';
export { ComponentId, AccountId, EnvironmentId } from './ids';
export { ParsedAgentId } from './agentId';
export type { ParsedAgentIdCreateOptions, ParsedAgentIdParts } from './agentId';
export * from './agentClassName';
export * from './newTypes/textInput';
export * from './newTypes/binaryInput';
export * from './newTypes/multimodalAdvanced';
export { Principal } from './principal';
export { AgentClassName } from './agentClassName';
export { CancellationToken } from 'golem:agent/host@2.0.0';
export { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
export * from './durableStreams';
export * from './webhook';
export * from './host/hostapi';
export * as oplog from './host/oplog';
export * from './host/guard';
export { acquireQuotaToken, QuotaToken, Reservation, withReservation } from './host/quota';
export type { FailedReservation } from './host/quota';
export type { GuestPermissionCardHandle as PermissionCard } from './internal/schema-model/permissionCardHandle';
export * from './host/retry';
export * from './host/result';
export * from './host/saga';
export * from './host/checkpoint';
export * from './host/durable';

export { defineAgent } from './defineAgent';
export { SnapshotError } from './snapshot';
export type { SnapshotPart, MultipartSnapshot } from './snapshot';
export { Snapshot } from './snapshot';
export { defineHttpRouter } from './defineHttpRouter';
export type {
  HttpRouterBuilder,
  HttpRouterOptions,
  HttpRouterContext,
  WebHttpRouterContext,
  HttpRouterHandler,
  RawHttpRouterHandler,
} from './defineHttpRouter';
export { withRawHeaders } from './httpRouterWeb';
export type {
  HttpRequest,
  HttpResponse,
  HttpHeader,
  FileExposure,
  FileResponseHeaders,
} from './httpRouterContract';
export type {
  AgentDefinition,
  MethodOnlyAgentClientDefinition,
  FullAgentClientDefinition,
  AgentImpl,
  AgentImplementation,
  AgentSpec,
  ConfigSpec,
  ConfigView,
  AgentContext,
  IdRecord,
  InitContext,
  SnapshotRestoreContext,
  MethodsRecord,
} from './defineAgent';
export { Secret } from './secret';
export { AgentStream } from './schema/agentStream';
export { schemaFingerprintV1, SchemaFingerprintError } from './internal/schema-model/fingerprint';
export type { SchemaGraph, SchemaType } from './internal/schema-model/model';
export { method } from './method';
export type { InputRecord, MethodSpec } from './method';
export type { StandardSchemaV1 } from './schema/standardSchema';
export { Bytes, KeyValue, Path, Quantity, s } from './schema/markers';
export type {
  KeyValueOptions,
  PathOptions,
  PermissionCardOptions,
  QuantityOptions,
} from './schema/markers';
export { registerSchemaWalker, registeredVendors, compileSchema } from './schema/adapter';
export type { SchemaCodec, SchemaWalker } from './schema/codec';
export { SchemaRef, SchemaRenderError } from './schema/ref';
export type { JsonValue, SchemaIssue, SchemaValidationResult } from './schema/ref';
export {
  c,
  command,
  err,
  ok,
  renderArgumentHelp,
  renderHelp,
  ToolInvokeError,
  toolDefinition,
  universalToolMiddleware,
} from './tool';
export { client, toolClientDefinition, ToolCallError } from './toolClient';
export type { ToolClientDefinition } from './toolClient';
export type {
  CamelCase,
  ConstraintRef,
  DocInput,
  ErrorOptions,
  FlagOptions,
  FormatterInput,
  GlobalCountFlagOptions,
  GlobalFlagOptions,
  GlobalValueOptions,
  ImplementedTool,
  ImplementedToolMiddleware,
  NestedCommandImplementation,
  OptionOptions,
  PositionalOptions,
  RepeatableMode,
  ReturnsOptions,
  StreamOptions,
  TailOptions,
  ToolBodyModel,
  ToolClient,
  ToolClientErrors,
  ToolClientInvocationResult,
  ToolClientMethod,
  ToolClientTransport,
  ToolCommandModel,
  ToolCommandModelOf,
  ToolConstraint,
  ToolDefinition,
  ToolErr,
  ToolHandler,
  ToolHelpError,
  ToolHelpResult,
  ToolImplementation,
  ToolInputStream,
  ToolInvocationContext,
  ToolInvokeErrorCause,
  ToolMiddlewareHandler,
  ToolMiddlewareImplementation,
  ToolMiddlewareInvocationContext,
  ToolMiddlewareOptions,
  ToolOk,
  ToolResult,
  ToolSubtreeModel,
  ToolUnderlying,
  ToolUnderlyingErrors,
  UniversalToolMiddlewareContext,
  UniversalToolMiddlewareInvocation,
  UniversalToolMiddlewareInvoke,
  UniversalToolMiddlewareOptions,
  UniversalToolUnderlying,
  UniversalToolUnderlyingInvoke,
} from './tool';
export { defineAgentClient, isRemoteCallError, RemoteCallError, RemoteOutputError } from './client';
export type { ToolCallErrorCause, ToolClientOptions } from './toolClient';
export type {
  FullAgentClientFactory,
  MethodOnlyAgentClientSpec,
  AgentConfigEntry,
  FullAgentClientSpec,
  ConfigOverrides,
  EphemeralInvocationResult,
  EphemeralRemoteClientFactory,
  PhantomClientDetails,
  RemoteAgentError,
  RemoteCallErrorCause,
  RemoteCallOptions,
  RemoteClient,
  RemoteClientFactory,
} from './client';
export {
  golemTool010ToolMiddlewareGuest,
  toolMiddlewareGuest,
} from './internal/tool/middlewareGuest';
export * from './keyvalue';
export * from './blobstore';
export * from './websocket';
export * from './rdbms';
export * as http from './http';
export * as bridge from './bridge';
export * as reflection from './reflection';
export {
  AgentMethod as ReflectedAgentMethodDefinition,
  AgentType as ReflectedAgentType,
  DynamicAgentClient,
  DynamicAgentMethod,
  ReflectedAgentClient,
  ReflectedAgentClientFactory,
  ReflectedAgentMethod,
  getAgentTypeByAgentId,
  getAllAgentTypes,
  getAgentType as getReflectedAgentType,
  getToolType as getReflectedToolType,
} from './reflection';
export type { ReflectedInvocation, ReflectedPhantomClient } from './reflection';
export type { CollectedToolInvocation, StartedToolInvocation } from './bridge/tool';
export { ToolStreamError } from './internal/tool/startedToolInvocation';

let initializedAgent: { agent: ResolvedAgent; principal: Principal } | undefined;

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
    stdin: AsyncIterable<ByteStreamItem> | undefined,
    stdout: ToolOutputWriter | undefined,
    stderr: ToolOutputWriter | undefined,
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
  if (initializedAgent) {
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

  const initiateResult = await initiator.initiate(input, principal);

  if (initiateResult.tag === 'ok') {
    initializedAgent = { agent: initiateResult.val, principal };
  } else {
    throw initiateResult.val;
  }
}

async function invokeAgent(
  methodName: string,
  input: SchemaValueTree,
  principal: Principal,
): Promise<SchemaValueTree | undefined> {
  if (!initializedAgent) {
    throw createCustomError(`Failed to invoke method ${methodName}: agent is not initialized`);
  }
  const result = await initializedAgent.agent.invoke(methodName, input, principal);

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
  stdin: AsyncIterable<ByteStreamItem> | undefined,
  stdout: ToolOutputWriter | undefined,
  stderr: ToolOutputWriter | undefined,
  principal: Principal,
): Promise<InvocationResult> {
  let inputAdapter: ToolInputStreamAdapter | undefined;
  let stdoutAdapter: ToolOutputStreamAdapter | undefined;
  let stderrAdapter: ToolOutputStreamAdapter | undefined;
  let inputCleanup: Promise<void> | undefined;
  const disposeInput = async (reason?: unknown): Promise<void> => {
    if (!inputCleanup) {
      inputCleanup = inputAdapter ? inputAdapter.dispose(reason) : closeAsyncIterable(stdin);
    }
    await inputCleanup;
  };

  try {
    const resolved = ToolRegistry.resolveInvocation(toolName, commandPath);

    let prepared;
    try {
      prepared = resolved.prepareWire(input);
    } catch (error) {
      throw invalidToolInput(`malformed invocation input: ${errorMessage(error)}`);
    }
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
      if (!stdout && body.stdout.required) {
        throw invalidToolInput('tool invocation did not contain declared stdout stream');
      }
      if (stdout) {
        stdoutAdapter = createToolOutputStream(stdout);
        context.stdout = stdoutAdapter.stream;
      }
    }

    if (body.stderr) {
      if (!stderr && body.stderr.required) {
        throw invalidToolInput('tool invocation did not contain declared stderr stream');
      }
      if (stderr) {
        stderrAdapter = createToolOutputStream(stderr);
        context.stderr = stderrAdapter.stream;
      }
    }

    const outcome = await prepared.invoke(context);
    await Promise.all([stdoutAdapter?.finish(), stderrAdapter?.finish()]);
    const result = await projectToolOutcome(body, outcome);
    await disposeInput();
    return result;
  } catch (error) {
    await Promise.allSettled([
      stdoutAdapter?.abort(error),
      stderrAdapter?.abort(error),
      disposeInput(error),
    ]);
    throw error;
  }
}

async function projectToolOutcome(
  body: ExtendedCommandBody,
  outcome: unknown,
): Promise<InvocationResult> {
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
      return { result: undefined };
    }
    return {
      result: await encodeToolValueAsync(body.result.codec, outcome.value, 'tool result'),
    };
  }

  if (outcome.tag === 'err') {
    if (!isDeclaredToolError(outcome)) {
      throw invalidToolResult('tool handler returned an invalid declared error');
    }
    const errorCase = body.errors.find((candidate) => candidate.name === outcome.name);
    if (!errorCase) {
      throw invalidToolResult(`tool handler returned undeclared error "${outcome.name}"`);
    }

    const payload = encodeDeclaredToolErrorPayload(
      errorCase,
      outcome,
      `tool error "${outcome.name}"`,
    );
    throw {
      tag: 'custom-error',
      val: { name: outcome.name, payload },
    } satisfies ToolError;
  }

  throw invalidToolResult(`tool handler returned unknown outcome tag "${outcome.tag}"`);
}

interface ToolInputStreamAdapter {
  readonly stream: ReadableStream<Uint8Array>;
  dispose(reason?: unknown): Promise<void>;
}

interface ToolOutputStreamAdapter {
  readonly stream: WritableStream<Uint8Array>;
  finish(): Promise<void>;
  abort(reason?: unknown): Promise<void>;
}

function readableStreamFromInput(input: AsyncIterable<ByteStreamItem>): ToolInputStreamAdapter {
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
  iterator: AsyncIterator<ByteStreamItem>,
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

    if (next.value.tag === 'err') {
      throw new ToolStreamError(next.value.val);
    }
    if (next.value.val.byteLength === 0) throw new TypeError('tool stdin yielded an empty chunk');
    controller.enqueue(next.value.val);
  } catch (error) {
    if (signal.aborted) closeReadableStream(controller);
    else controller.error(error);
  }
}

function createToolOutputStream(writer: ToolOutputWriter): ToolOutputStreamAdapter {
  const invocationCompleted = new Error('tool invocation completed');
  let activeOperation: Promise<void> | undefined;
  let controller: WritableStreamDefaultController | undefined;
  let acceptingOperations = true;
  let terminated = false;
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
    acceptingOperations = false;
    controller?.error(abortReason);
    if (!terminated) {
      terminated = true;
      try {
        await writer.fail(toolStreamFailureFromError(abortReason));
      } catch {
        // An endpoint terminal selected by the handler remains authoritative.
      }
    }
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
          if (contents.byteLength === 0) return;
          return writer.write(contents);
        }),
      );
    },
    close() {
      if (!acceptingOperations) return Promise.reject(failed ? failure : invocationCompleted);
      acceptingOperations = false;
      terminated = true;
      return track(writer.finish());
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
      if (!terminated) {
        terminated = true;
        try {
          await writer.finish();
        } catch {
          // The writer's selected or drop terminal reports completion failure independently.
        }
      }
    },
    abort,
  };
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
  if (!initializedAgent) {
    throw new Error('Failed to get agent definition: agent is not initialized');
  }

  return initializedAgent.agent.getAgentType();
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

function deserializePrincipal(obj: any, strict = false): Principal {
  if (strict) {
    if (obj === null || typeof obj !== 'object' || Array.isArray(obj))
      throw new SnapshotError('invalid snapshot principal');
    if (obj.tag !== 'anonymous') {
      if (obj.val === null || typeof obj.val !== 'object' || Array.isArray(obj.val))
        throw new SnapshotError('invalid snapshot principal value');
      const val = obj.val;
      if (
        obj.tag === 'agent' &&
        (typeof val.componentId !== 'string' || typeof val.agentId !== 'string')
      )
        throw new SnapshotError('invalid agent principal');
      if (obj.tag === 'golem-user' && typeof val.accountId !== 'string')
        throw new SnapshotError('invalid golem-user principal');
      if (obj.tag === 'oidc') {
        for (const key of ['sub', 'issuer', 'claims']) {
          if (typeof val[key] !== 'string')
            throw new SnapshotError(`invalid oidc principal ${key}`);
        }
        for (const key of [
          'email',
          'name',
          'givenName',
          'familyName',
          'picture',
          'preferredUsername',
        ]) {
          if (val[key] != null && typeof val[key] !== 'string')
            throw new SnapshotError(`invalid oidc principal ${key}`);
        }
        if (val.emailVerified != null && typeof val.emailVerified !== 'boolean')
          throw new SnapshotError('invalid oidc principal emailVerified');
      }
    }
  }
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
  if (!initializedAgent) {
    throw new Error('Failed to save agent snapshot: agent is not initialized');
  }

  const transport = await initializedAgent.agent.saveSnapshot();
  const principal = initializedAgent.principal;
  const serializedPrincipal = serializePrincipal(principal);

  if (transport.kind === 'multipart') {
    const envelope = {
      version: 1,
      principal: serializedPrincipal,
      state: transport.state,
      ...(transport.fileDatabases === undefined ? {} : { fileDatabases: transport.fileDatabases }),
    };
    const { data, boundary } = encodeMultipart([
      {
        name: 'state',
        contentType: 'application/json',
        body: new TextEncoder().encode(JSON.stringify(envelope)),
      },
      ...transport.parts,
    ]);
    return {
      payload: data,
      mimeType: `multipart/mixed; boundary=${boundary}`,
    };
  } else if (transport.kind === 'json') {
    // JSON snapshot: wrap typed state and database metadata in its envelope.
    const state = JSON.parse(new TextDecoder().decode(transport.data));
    const envelope = {
      version: 1,
      principal: serializedPrincipal,
      state,
      fileDatabases: transport.fileDatabases,
    };
    return {
      payload: new TextEncoder().encode(JSON.stringify(envelope)),
      mimeType: 'application/json',
    };
  } else {
    // Binary snapshot: version-2 binary envelope with principal
    const principalJson = JSON.stringify(serializedPrincipal);
    const principalBytes = new TextEncoder().encode(principalJson);

    const totalLength = 1 + 4 + principalBytes.length + transport.data.length;
    const fullSnapshot = new Uint8Array(totalLength);
    const view = new DataView(fullSnapshot.buffer);
    view.setUint8(0, 2); // version
    view.setUint32(1, principalBytes.length, false); // big-endian
    fullSnapshot.set(principalBytes, 5);
    fullSnapshot.set(transport.data, 5 + principalBytes.length);

    return { payload: fullSnapshot, mimeType: 'application/octet-stream' };
  }
}

async function load(snapshot: { payload: Uint8Array; mimeType: string }): Promise<void> {
  const bytes = snapshot.payload;

  if (initializedAgent) {
    throw `Agent is already initialized in this container`;
  }

  const [agentTypeName, agentParameters] = getRawSelfAgentId().parsedWire();
  const registrationError = AgentTypeRegistry.getRegistrationError(agentTypeName);
  if (registrationError) {
    // The snapshot WIT interface returns `result<_, string>`, not AgentError.
    throw formatAgentRegistrationError(agentTypeName, registrationError);
  }

  let agentSnapshot: Uint8Array;
  let agentSnapshotMimeType: string | undefined;
  let principal: Principal;
  let databases: SnapshotDatabases | undefined = { inMemory: [], fileDatabases: {} };
  const userParts = new Map<string, SnapshotPart>();

  const decodeJsonEnvelope = (data: Uint8Array, description: string, strict = false) => {
    const text = new TextDecoder().decode(data);
    const envelope = JSON.parse(text);
    if (strict) {
      const encoded = new TextEncoder().encode(text);
      if (encoded.length !== data.length || encoded.some((byte, i) => byte !== data[i])) {
        throw new SnapshotError('multipart state must be valid UTF-8');
      }
      const tokens = text.match(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^{}[\],:\s]+/g)!;
      const objects: Array<Set<string> | undefined> = [];
      for (let i = 0; i < tokens.length; i++) {
        const token = tokens[i];
        if (token === '{') objects.push(new Set());
        else if (token === '[') objects.push(undefined);
        else if (token === '}' || token === ']') objects.pop();
        else if (token.startsWith('"') && tokens[i + 1] === ':') {
          const key: string = JSON.parse(token);
          const keys = objects[objects.length - 1]!;
          if (keys.has(key)) throw new SnapshotError(`duplicate JSON key '${key}'`);
          keys.add(key);
          if (objects.length === 1 && key === 'version' && tokens[i + 2] !== '1') {
            throw new SnapshotError('multipart version must be integer 1');
          }
        }
      }
    }
    if (envelope === null || typeof envelope !== 'object' || Array.isArray(envelope)) {
      throw `${description} must be a JSON object`;
    }
    if (!Object.hasOwn(envelope, 'version')) {
      throw `${description} missing 'version' field`;
    }
    if (envelope.version !== 1) {
      throw `${description} version must be 1`;
    }
    if (!Object.hasOwn(envelope, 'principal')) {
      throw `${description} missing 'principal' field`;
    }
    if (!Object.hasOwn(envelope, 'state')) {
      throw `${description} missing 'state' field`;
    }
    return envelope;
  };

  if (snapshot.mimeType.split(';', 1)[0].trim().toLowerCase() === 'multipart/mixed') {
    // Multipart snapshot: extract principal from the state JSON part
    const boundary = extractBoundary(snapshot.mimeType);
    if (!boundary) throw new SnapshotError('multipart snapshot missing boundary');
    const parts = decodeMultipart(bytes, boundary);

    const stateIdx = parts.findIndex((p) => p.name === 'state');
    if (stateIdx === -1) {
      throw 'multipart snapshot missing "state" part';
    }

    if (parts[stateIdx].contentType !== 'application/json')
      throw new SnapshotError('state part must be application/json');
    const envelope = decodeJsonEnvelope(parts[stateIdx].body, 'multipart state part', true);
    principal = deserializePrincipal(envelope.principal, true);

    agentSnapshot = new TextEncoder().encode(JSON.stringify(envelope.state));
    agentSnapshotMimeType = 'multipart/mixed';
    for (const part of parts) {
      if (part.name === 'state') continue;
      if (part.name.startsWith('part:')) {
        const name = part.name.slice(5);
        validatePartName(name);
        userParts.set(name, {
          bytes: part.body,
          contentType: normalizeContentType(part.contentType),
        });
      } else if (part.name.startsWith('db:')) {
        if (part.contentType !== 'application/x-sqlite3')
          throw new SnapshotError('database part must be application/x-sqlite3');
      } else throw new SnapshotError(`unknown snapshot namespace '${part.name}'`);
    }
    databases =
      Object.hasOwn(envelope, 'fileDatabases') || parts.some((part) => part.name.startsWith('db:'))
        ? decodeSnapshotDatabases(parts, envelope, 'multipart state part')
        : undefined;
  } else if (snapshot.mimeType === 'application/json') {
    // JSON snapshot: unwrap envelope { version, principal, state, fileDatabases }
    const envelope = decodeJsonEnvelope(bytes, 'JSON snapshot');
    principal = deserializePrincipal(envelope.principal);
    agentSnapshot = new TextEncoder().encode(JSON.stringify(envelope.state));
    agentSnapshotMimeType = 'application/json';
    databases = decodeSnapshotDatabases([], envelope, 'JSON snapshot');
  } else {
    // Custom binary snapshot with version envelope
    if (bytes.byteLength < 1) {
      throw `Snapshot is empty`;
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const version = view.getUint8(0);

    if (version === 1) {
      agentSnapshot = bytes.slice(1);
      principal = { tag: 'anonymous' };
    } else if (version === 2) {
      if (bytes.byteLength < 5) {
        throw `Version 2 snapshot too short for principal length`;
      }
      const principalLen = view.getUint32(1, false); // big-endian
      if (principalLen > bytes.byteLength - 5) {
        throw `Version 2 snapshot too short for principal data`;
      }
      const principalBytes = bytes.slice(5, 5 + principalLen);
      principal = deserializePrincipal(JSON.parse(new TextDecoder().decode(principalBytes)));
      agentSnapshot = bytes.slice(5 + principalLen);
    } else {
      throw `Unsupported snapshot version ${version}`;
    }
  }

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  const initiateResult = await initiator.loadSnapshot(
    agentParameters,
    principal,
    agentSnapshot,
    agentSnapshotMimeType,
    databases,
    userParts,
  );

  if (initiateResult.tag === 'ok') {
    initializedAgent = { agent: initiateResult.val, principal };
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
