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

import WebSocket, { type RawData } from 'ws';

export type PhantomId = string;

export type GolemServer =
  | { type: 'local' }
  | { type: 'cloud'; token: string }
  | { type: 'custom'; url: string; token: string };

export const LOCAL_WELL_KNOWN_TOKEN = '5c832d93-ff85-4a8f-9803-513950fdfdb1';

export type AroundInvokeHook = {
  beforeInvoke: (request: AgentInvocationRequest) => Promise<void>;
  afterInvoke: (
    request: AgentInvocationRequest,
    result: JsonResult<AgentInvocationResult, any>,
  ) => Promise<void>;
};

export type Configuration = {
  server: GolemServer;
  application: ApplicationName;
  environment: EnvironmentName;
  aroundInvokeHook?: AroundInvokeHook;
};

export type ApplicationName = string;
export type EnvironmentName = string;
export type AgentTypeName = string;
export type IdempotencyKey = string;

/** Binary rich value used by the public streaming protocol. */
export interface AgentBinary {
  bytes: Uint8Array;
  mimeType?: string;
}

/** Text rich value used by the public streaming protocol. */
export interface AgentText {
  text: string;
  language?: string;
}

/** A single-consumer asynchronous stream returned by a streaming agent method. */
export interface AgentStream<T> extends AsyncIterable<T> {}

/** Wraps an async iterable with the affine `AgentStream` ownership contract. */
export function agentStream<T>(source: AsyncIterable<T>): AgentStream<T> {
  let claimed = false;
  return {
    [Symbol.asyncIterator](): AsyncIterator<T> {
      if (claimed) throw new Error('AgentStream can only be iterated once');
      claimed = true;
      return source[Symbol.asyncIterator]();
    },
  };
}

export type PublicValue =
  | null
  | boolean
  | number
  | string
  | PublicValue[]
  | {
      [key: string]: PublicValue;
    };

export type NumericBound =
  | { tag: 'signed'; val: bigint }
  | { tag: 'unsigned'; val: bigint }
  | { tag: 'float-bits'; val: bigint };

export interface NumericRestrictions {
  min?: NumericBound;
  max?: NumericBound;
  unit?: string;
}

export interface TextRestrictions {
  languages?: string[];
  minLength?: number;
  maxLength?: number;
  regex?: string;
}

export interface BinaryRestrictions {
  mimeTypes?: string[];
  minBytes?: number;
  maxBytes?: number;
}

export interface PathSpec {
  direction: 'input' | 'output' | 'in-out';
  kind: 'file' | 'directory' | 'any';
  allowedMimeTypes?: string[];
  allowedExtensions?: string[];
}

export interface UrlRestrictions {
  allowedSchemes?: string[];
  allowedHosts?: string[];
}

export interface QuantityValue {
  mantissa: bigint;
  scale: number;
  unit: string;
}

export interface QuantitySpec {
  baseUnit: string;
  allowedSuffixes: string[];
  min?: QuantityValue;
  max?: QuantityValue;
}

export type DiscriminatorRule =
  | { tag: 'prefix'; val: string }
  | { tag: 'suffix'; val: string }
  | { tag: 'contains'; val: string }
  | { tag: 'regex'; val: string }
  | { tag: 'field-equals'; val: { fieldName: string; literal?: string } }
  | { tag: 'field-absent'; val: string };

export interface MetadataEnvelope {
  doc?: string;
  aliases: string[];
  examples: string[];
  deprecated?: string;
  role?:
    | { tag: 'multimodal' }
    | { tag: 'unstructured-text' }
    | { tag: 'unstructured-binary' }
    | { tag: 'other'; val: string };
}

export interface NamedFieldType {
  name: string;
  body: SchemaType;
  metadata: MetadataEnvelope;
}

export interface VariantCaseType {
  name: string;
  payload?: SchemaType;
  metadata: MetadataEnvelope;
}

export interface UnionBranch {
  tag: string;
  body: SchemaType;
  discriminator: DiscriminatorRule;
  metadata: MetadataEnvelope;
}

export type SchemaTypeBody =
  | { tag: 'ref'; id: string }
  | { tag: 'bool' }
  | { tag: 's8'; restrictions?: NumericRestrictions }
  | { tag: 's16'; restrictions?: NumericRestrictions }
  | { tag: 's32'; restrictions?: NumericRestrictions }
  | { tag: 's64'; restrictions?: NumericRestrictions }
  | { tag: 'u8'; restrictions?: NumericRestrictions }
  | { tag: 'u16'; restrictions?: NumericRestrictions }
  | { tag: 'u32'; restrictions?: NumericRestrictions }
  | { tag: 'u64'; restrictions?: NumericRestrictions }
  | { tag: 'f32'; restrictions?: NumericRestrictions }
  | { tag: 'f64'; restrictions?: NumericRestrictions }
  | { tag: 'char' }
  | { tag: 'string' }
  | { tag: 'record'; fields: NamedFieldType[] }
  | { tag: 'variant'; cases: VariantCaseType[] }
  | { tag: 'enum'; cases: string[] }
  | { tag: 'flags'; names: string[] }
  | { tag: 'tuple'; elements: SchemaType[] }
  | { tag: 'list'; element: SchemaType }
  | { tag: 'fixed-list'; element: SchemaType; length: number }
  | { tag: 'map'; key: SchemaType; value: SchemaType }
  | { tag: 'option'; element: SchemaType }
  | { tag: 'result'; ok?: SchemaType; err?: SchemaType }
  | { tag: 'text'; restrictions: TextRestrictions }
  | { tag: 'binary'; restrictions: BinaryRestrictions }
  | { tag: 'path'; spec: PathSpec }
  | { tag: 'url'; restrictions: UrlRestrictions }
  | { tag: 'datetime' }
  | { tag: 'duration' }
  | { tag: 'quantity'; spec: QuantitySpec }
  | { tag: 'union'; branches: UnionBranch[] }
  | { tag: 'secret'; spec: { category?: string }; inner: SchemaType }
  | { tag: 'quota-token'; spec: { resourceName?: string } }
  | { tag: 'permission-card'; spec: { polymorphic: boolean } }
  | { tag: 'future'; element?: SchemaType }
  | { tag: 'stream'; element?: SchemaType };

export interface SchemaType {
  body: SchemaTypeBody;
  metadata: MetadataEnvelope;
}

export interface SchemaTypeDef {
  name?: string;
  body: SchemaType;
}

export interface SchemaGraph {
  defs: ReadonlyMap<string, SchemaTypeDef>;
  root: SchemaType;
}

export function schemaType(
  body: SchemaTypeBody,
  metadata: MetadataEnvelope = { aliases: [], examples: [] },
): SchemaType {
  return { body, metadata };
}

export interface StreamingInvocationDescriptor {
  application: string;
  environment: string;
  agentType: string;
  constructorParameters: PublicValue;
  phantomId?: string;
  config: Array<{ path: string[]; value: PublicValue }>;
  method: string;
  idempotencyKey?: string;
}

export type StreamingRemoteMethod<Args extends any[], R> = {
  (...args: Args): Promise<R>;
  abortable(signal: AbortSignal, ...args: Args): Promise<R>;
};

// ===========================================================================
// Schema-native wire values
//
// These mirror the Rust `SchemaValue` / `TypedSchemaValue` serde shapes
// (`#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]`).
// Request `parameters` / `methodParameters` and agent `config` values travel
// as a bare `SchemaValue`; invocation results come back as a `TypedSchemaValue`.
// ===========================================================================

export interface TextValuePayload {
  text: string;
  language?: string;
}

export interface BinaryValuePayload {
  bytes: number[];
  mimeType?: string;
}

/**
 * Schema-native value, mirroring the server's Rust `SchemaValue`. The wire
 * form is the serde derive of `enum SchemaValue` with `tag = "kind"` /
 * `content = "value"` and kebab-cased discriminants. Composite payloads are
 * positional and driven by the schema (records carry no field names, variants
 * carry a `case` index, etc.).
 */
export type SchemaValue =
  | { kind: 'bool'; value: boolean }
  | { kind: 's8'; value: number }
  | { kind: 's16'; value: number }
  | { kind: 's32'; value: number }
  | { kind: 's64'; value: number | bigint }
  | { kind: 'u8'; value: number }
  | { kind: 'u16'; value: number }
  | { kind: 'u32'; value: number }
  | { kind: 'u64'; value: number | bigint }
  | { kind: 'f32'; value: number }
  | { kind: 'f64'; value: number }
  | { kind: 'char'; value: string }
  | { kind: 'string'; value: string }
  | { kind: 'record'; value: { fields: SchemaValue[] } }
  | { kind: 'variant'; value: { case: number; payload?: SchemaValue } }
  | { kind: 'enum'; value: { case: number } }
  | { kind: 'flags'; value: { bits: boolean[] } }
  | { kind: 'tuple'; value: { elements: SchemaValue[] } }
  | { kind: 'list'; value: { elements: SchemaValue[] } }
  | { kind: 'fixed-list'; value: { elements: SchemaValue[] } }
  | { kind: 'map'; value: { entries: [SchemaValue, SchemaValue][] } }
  | { kind: 'option'; value: { inner?: SchemaValue } }
  | { kind: 'result'; value: { tag: 'ok' | 'err'; value?: SchemaValue } }
  | { kind: 'text'; value: TextValuePayload }
  | { kind: 'binary'; value: BinaryValuePayload }
  | { kind: 'path'; value: { path: string } }
  | { kind: 'url'; value: { url: string } }
  | { kind: 'datetime'; value: { value: string } }
  | { kind: 'duration'; value: { nanoseconds: number | bigint } }
  | { kind: 'quantity'; value: QuantityValue }
  | { kind: 'union'; value: { tag: string; body: SchemaValue } };

/**
 * A self-contained schema graph paired with a value (the server's Rust
 * `TypedSchemaValue`). Generated clients decode `value` guided by their static
 * schema and do not need to interpret `graph`.
 */
export interface TypedSchemaValue {
  graph: unknown;
  value: SchemaValue;
}

export type AgentInvocationMode = 'await' | 'schedule';

export interface AgentInvocationRequest {
  appName: ApplicationName;
  envName: EnvironmentName;
  agentTypeName: AgentTypeName;
  parameters: SchemaValue;
  phantomId?: PhantomId;
  config?: AgentConfigEntry[];
  methodName: string;
  methodParameters: SchemaValue;
  mode: AgentInvocationMode;
  scheduleAt?: string; // ISO 8601 datetime
  idempotencyKey?: IdempotencyKey;
}

export interface AgentInvocationResult {
  agentId: AgentId;
  idempotencyKey: IdempotencyKey;
  result?: TypedSchemaValue;
  componentRevision?: number;
}

export interface InvocationMetadata {
  agentId: AgentId;
  idempotencyKey: IdempotencyKey;
}

export interface InvocationResult<T> {
  metadata: InvocationMetadata;
  value: T;
}

export interface InvocationReceipt {
  metadata: InvocationMetadata;
}

export interface AgentConfigEntry {
  path: string[];
  value: SchemaValue;
}

export interface CreateAgentRequest {
  appName: ApplicationName;
  envName: EnvironmentName;
  agentTypeName: AgentTypeName;
  parameters: SchemaValue;
  phantomId?: PhantomId;
  config?: AgentConfigEntry[];
}

export interface AgentId {
  componentId: string;
  agentId: string;
}

export interface CreateAgentResponse {
  agentId: AgentId;
  componentRevision: number;
}

export interface GolemAgentErrorDetails {
  cause: string;
  stderr: string;
}

export type GolemErrorBody =
  | { code: string; error: string; agentError?: GolemAgentErrorDetails }
  | { code: string; errors: string[] };

export class GolemServiceError extends Error {
  readonly operation: 'createAgent' | 'invokeAgent';
  readonly status: number;
  readonly statusText: string;
  readonly bodyText?: string;
  readonly body?: GolemErrorBody;

  constructor(params: {
    operation: 'createAgent' | 'invokeAgent';
    status: number;
    statusText: string;
    bodyText?: string;
    body?: GolemErrorBody;
  }) {
    super(formatGolemServiceErrorMessage(params));
    this.operation = params.operation;
    this.status = params.status;
    this.statusText = params.statusText;
    this.bodyText = params.bodyText;
    this.body = params.body;

    Object.defineProperties(this, {
      name: { value: 'GolemServiceError', enumerable: false, configurable: true },
      operation: { value: params.operation, enumerable: false, configurable: true },
      status: { value: params.status, enumerable: false, configurable: true },
      statusText: { value: params.statusText, enumerable: false, configurable: true },
      bodyText: { value: params.bodyText, enumerable: false, configurable: true },
      body: { value: params.body, enumerable: false, configurable: true },
    });
  }
}

function formatGolemServiceErrorMessage(params: {
  operation: 'createAgent' | 'invokeAgent';
  status: number;
  statusText: string;
  bodyText?: string;
  body?: GolemErrorBody;
}): string {
  const action = params.operation === 'createAgent' ? 'Agent creation' : 'Agent invocation';
  const status = [params.status, params.statusText].filter(Boolean).join(' ');
  const lines = [`${action} failed: ${status}`];

  if (params.body) {
    if ('errors' in params.body) {
      lines.push(`Code: ${params.body.code}`);
      lines.push('Messages:');
      lines.push(...params.body.errors.map((error) => `- ${error}`));
      return lines.join('\n');
    }

    lines.push(`Code: ${params.body.code}`);
    lines.push(`Message: ${params.body.error}`);
    appendAgentErrorMessage(lines, params.body.agentError);
    return lines.join('\n');
  }

  if (params.bodyText) {
    lines.push(...formatResponseBodyFallback(params.bodyText));
  }

  return lines.join('\n');
}

function formatResponseBodyFallback(bodyText: string): string[] {
  const trimmed = bodyText.trim();
  if (!trimmed) return [];

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return ['', 'Response body:', trimmed];
  }

  if (isRecord(parsed)) {
    if (typeof parsed.message === 'string') {
      return ['', `Response message: ${parsed.message}`];
    }
    if (typeof parsed.error === 'string') {
      return ['', `Response message: ${parsed.error}`];
    }
    if (Array.isArray(parsed.errors) && parsed.errors.every((error) => typeof error === 'string')) {
      return ['', 'Response messages:', ...parsed.errors.map((error) => `- ${error}`)];
    }

    const title = typeof parsed.title === 'string' ? parsed.title : undefined;
    const detail = typeof parsed.detail === 'string' ? parsed.detail : undefined;
    if (title || detail) {
      return [
        '',
        ...(title ? [`Response title: ${title}`] : []),
        ...(detail ? [`Response detail: ${detail}`] : []),
      ];
    }
  }

  return ['', 'Response body:', JSON.stringify(parsed, null, 2)];
}

function appendAgentErrorMessage(lines: string[], agentError: GolemAgentErrorDetails | undefined) {
  if (!agentError) return;

  const stderr = trimEmptyLines(agentError.stderr.split('\n'));
  if (stderr.length > 0) {
    lines.push('');
    lines.push('Stderr:');
    lines.push(...stderr);
  }

  const trap = extractWasmTrap(agentError.cause);
  if (trap) {
    lines.push('');
    lines.push(`Wasm trap: ${trap}`);
  }
}

function extractWasmTrap(cause: string): string | undefined {
  const trapLine = trimEmptyLines(cause.split('\n'))
    .reverse()
    .find((line) => line.includes('wasm trap:'));
  return trapLine?.split('wasm trap:').pop()?.trim();
}

function trimEmptyLines(lines: string[]): string[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === '') start += 1;
  while (end > start && lines[end - 1].trim() === '') end -= 1;
  return lines.slice(start, end);
}

async function throwGolemServiceError(
  operation: 'createAgent' | 'invokeAgent',
  response: Response,
): Promise<never> {
  const bodyText = await response.text().catch(() => undefined);
  throw new GolemServiceError({
    operation,
    status: response.status,
    statusText: response.statusText,
    bodyText,
    body: parseGolemErrorBody(bodyText),
  });
}

function parseGolemErrorBody(bodyText: string | undefined): GolemErrorBody | undefined {
  if (!bodyText) return undefined;

  let parsed: unknown;
  try {
    parsed = JSON.parse(bodyText);
  } catch {
    return undefined;
  }

  if (!isRecord(parsed) || typeof parsed.code !== 'string') {
    return undefined;
  }

  if (Array.isArray(parsed.errors) && parsed.errors.every((error) => typeof error === 'string')) {
    return { code: parsed.code, errors: parsed.errors };
  }

  if (typeof parsed.error !== 'string') {
    return undefined;
  }

  const agentError = parseAgentErrorDetails(parsed.workerError);
  return agentError
    ? { code: parsed.code, error: parsed.error, agentError }
    : { code: parsed.code, error: parsed.error };
}

function parseAgentErrorDetails(value: unknown): GolemAgentErrorDetails | undefined {
  if (!isRecord(value)) return undefined;
  if (typeof value.cause !== 'string' || typeof value.stderr !== 'string') return undefined;
  return { cause: value.cause, stderr: value.stderr };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function restJson(value: unknown): string {
  const encode = (current: unknown, arrayElement: boolean): string | undefined => {
    if (current === null) return 'null';
    switch (typeof current) {
      case 'boolean':
      case 'string':
        return JSON.stringify(current);
      case 'bigint':
        return current.toString();
      case 'number':
        return Number.isFinite(current) ? JSON.stringify(current) : 'null';
      case 'object':
        if (Array.isArray(current)) {
          return `[${current.map((item) => encode(item, true) ?? 'null').join(',')}]`;
        }
        return `{${Object.keys(current)
          .flatMap((key) => {
            const encoded = encode((current as Record<string, unknown>)[key], false);
            return encoded === undefined ? [] : [`${JSON.stringify(key)}:${encoded}`];
          })
          .join(',')}}`;
      case 'undefined':
      case 'function':
      case 'symbol':
        return arrayElement ? 'null' : undefined;
    }
  };
  return encode(value, false) ?? 'null';
}

export async function createAgent(
  server: GolemServer,
  request: CreateAgentRequest,
): Promise<CreateAgentResponse> {
  let baseUrl: string;
  let token: string;

  switch (server.type) {
    case 'local':
      baseUrl = 'http://localhost:9881';
      token = LOCAL_WELL_KNOWN_TOKEN;
      break;
    case 'cloud':
      baseUrl = 'https://release.api.golem.cloud';
      token = server.token;
      break;
    case 'custom':
      baseUrl = server.url;
      token = server.token;
      break;
  }

  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };

  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const rawResponse = await fetch(`${baseUrl}/v1/agents/create-agent`, {
    method: 'POST',
    headers,
    body: restJson(request),
  });

  if (!rawResponse.ok) {
    await throwGolemServiceError('createAgent', rawResponse);
  }

  return await (rawResponse.json() as Promise<CreateAgentResponse>);
}

function throwIfAborted(signal?: AbortSignal): void {
  if (!signal?.aborted) return;

  if (signal.reason !== undefined) {
    throw signal.reason;
  }

  const err = new Error('The operation was aborted.');
  err.name = 'AbortError';
  throw err;
}

export async function invokeAgent(
  server: GolemServer,
  request: AgentInvocationRequest,
  aroundInvokeHook: AroundInvokeHook | undefined = undefined,
  signal?: AbortSignal,
): Promise<AgentInvocationResult> {
  throwIfAborted(signal);

  let baseUrl: string;
  let token: string;

  switch (server.type) {
    case 'local':
      baseUrl = 'http://localhost:9881';
      token = LOCAL_WELL_KNOWN_TOKEN;
      break;
    case 'cloud':
      baseUrl = 'https://release.api.golem.cloud';
      token = server.token;
      break;
    case 'custom':
      baseUrl = server.url;
      token = server.token;
      break;
  }

  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };

  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  if (request.idempotencyKey) {
    headers['Idempotency-Key'] = request.idempotencyKey!;
  }

  if (aroundInvokeHook) {
    await aroundInvokeHook.beforeInvoke(request);
  }

  throwIfAborted(signal);

  try {
    const rawResponse = await fetch(`${baseUrl}/v1/agents/invoke-agent`, {
      method: 'POST',
      headers,
      body: restJson(request),
      signal,
    });

    if (!rawResponse.ok) {
      await throwGolemServiceError('invokeAgent', rawResponse);
    }

    let response = await (rawResponse.json() as Promise<AgentInvocationResult>);

    if (aroundInvokeHook) {
      await aroundInvokeHook.afterInvoke(request, { ok: response });
    }

    return response;
  } catch (e) {
    await aroundInvokeHook?.afterInvoke(request, { err: e });
    throw e;
  }
}

/// The Result type representation in Golem's JSON type mapping
export type JsonResult<Ok, Err> = { ok: Ok; err?: undefined } | { ok?: undefined; err: Err };

export type RemoteMethod<Args extends any[], R> = {
  (...args: Args): Promise<R>;
  /**
   * Invoke the remote method with abort support. When the signal is aborted,
   * the HTTP request is cancelled and the promise rejects.
   *
   * **Important:** Aborting cancels the HTTP request but the remote agent
   * may still execute the invoked method if the request was already dispatched.
   */
  abortable: (signal: AbortSignal, ...args: Args) => Promise<R>;
  trigger: (...args: Args) => void;
  schedule: (scheduleAt: string, ...args: Args) => void;
};

export type EphemeralRemoteMethod<Args extends any[], R> = {
  (...args: Args): Promise<InvocationResult<R>>;
  abortable: (signal: AbortSignal, ...args: Args) => Promise<InvocationResult<R>>;
  trigger: (...args: Args) => Promise<InvocationReceipt>;
  schedule: (scheduleAt: string, ...args: Args) => Promise<InvocationReceipt>;
};

export function createRemoteMethod<Args extends any[], R>(
  getServer: () => GolemServer,
  aroundInvokeHook: () => AroundInvokeHook | undefined,
  getRequest: () => AgentInvocationRequest,
  encode: (args: Args) => SchemaValue,
  decode: (result: AgentInvocationResult) => R,
): RemoteMethod<Args, R> {
  const result = async function (...args: Args): Promise<R> {
    const invokeResult = await invokeAgent(
      getServer(),
      {
        ...getRequest(),
        methodParameters: encode(args),
        mode: 'await',
        scheduleAt: undefined,
      },
      aroundInvokeHook(),
    );
    return decode(invokeResult);
  };
  result.trigger = function (...args: Args): void {
    void invokeAgent(getServer(), {
      ...getRequest(),
      methodParameters: encode(args),
      mode: 'schedule',
      scheduleAt: undefined,
    });
  };
  result.schedule = function (scheduleAt: string, ...args: Args): void {
    void invokeAgent(getServer(), {
      ...getRequest(),
      methodParameters: encode(args),
      mode: 'schedule',
      scheduleAt,
    });
  };
  result.abortable = async function (signal: AbortSignal, ...args: Args): Promise<R> {
    throwIfAborted(signal);

    const invokeResult = await invokeAgent(
      getServer(),
      {
        ...getRequest(),
        methodParameters: encode(args),
        mode: 'await',
        scheduleAt: undefined,
      },
      aroundInvokeHook(),
      signal,
    );
    return decode(invokeResult);
  };
  return result;
}

export function createEphemeralRemoteMethod<Args extends any[], R>(
  getServer: () => GolemServer,
  aroundInvokeHook: () => AroundInvokeHook | undefined,
  getRequest: () => AgentInvocationRequest,
  encode: (args: Args) => SchemaValue,
  decode: (result: AgentInvocationResult) => R,
): EphemeralRemoteMethod<Args, R> {
  const invoke = async (
    args: Args,
    mode: AgentInvocationMode,
    scheduleAt: string | undefined,
    signal?: AbortSignal,
  ): Promise<AgentInvocationResult> =>
    invokeAgent(
      getServer(),
      {
        ...getRequest(),
        methodParameters: encode(args),
        mode,
        scheduleAt,
      },
      aroundInvokeHook(),
      signal,
    );

  const metadata = (response: AgentInvocationResult): InvocationMetadata => ({
    agentId: response.agentId,
    idempotencyKey: response.idempotencyKey,
  });

  const result = async function (...args: Args): Promise<InvocationResult<R>> {
    const response = await invoke(args, 'await', undefined);
    return { metadata: metadata(response), value: decode(response) };
  };
  result.abortable = async function (
    signal: AbortSignal,
    ...args: Args
  ): Promise<InvocationResult<R>> {
    throwIfAborted(signal);
    const response = await invoke(args, 'await', undefined, signal);
    return { metadata: metadata(response), value: decode(response) };
  };
  result.trigger = async function (...args: Args): Promise<InvocationReceipt> {
    const response = await invoke(args, 'schedule', undefined);
    return { metadata: metadata(response) };
  };
  result.schedule = async function (scheduleAt: string, ...args: Args): Promise<InvocationReceipt> {
    const response = await invoke(args, 'schedule', scheduleAt);
    return { metadata: metadata(response) };
  };
  return result;
}

type LanguageCode = string;

/**
 * Represents unstructured text input, which can be either a URL or inline text.
 *
 * Example usage:
 *
 * ```ts
 *
 * function foo(input: UnstructuredText) {..}
 *
 * // With language codes
 * function bar(input: UnstructuredText<['en', 'de']>) {..}
 *
 *
 * foo(UnstructuredText.fromInline("hello"));
 *
 * bar(UnstructuredText.fromInline("hello", 'en')); // with language code
 *
 * ```
 */
export type UnstructuredText<LC extends LanguageCode[] = []> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: string;
      languageCode?: LC[number];
    };

export type UnstructuredTextType<LC extends LanguageCode[] = []> = UnstructuredText<LC>;

// Variant case indices of the canonical role-marked unstructured wrapper:
// `variant { inline: text/binary, url: url }`.
const UNSTRUCTURED_INLINE_CASE = 0;
const UNSTRUCTURED_URL_CASE = 1;

export const UnstructuredText = {
  /**
   * Decodes a schema-native unstructured-text `variant { inline, url }` value
   * into an `UnstructuredText`, validating the language tag against
   * `allowedCodes` when the agent declares a fixed set.
   */
  fromSchemaValue<LC extends string[] = []>(
    parameterName: string,
    value: SchemaValue,
    allowedCodes: string[],
  ): UnstructuredText<LC> {
    if (value.kind !== 'variant') {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Expected an unstructured-text 'variant' value, got '${value.kind}'`,
      );
    }
    const { case: caseIndex, payload } = value.value;
    if (caseIndex === UNSTRUCTURED_URL_CASE) {
      if (!payload || payload.kind !== 'url') {
        throw new Error(
          `Invalid value for parameter ${parameterName}. Expected a 'url' payload for the unstructured-text url case`,
        );
      }
      return { tag: 'url', val: payload.value.url } as UnstructuredText<LC>;
    }
    if (caseIndex !== UNSTRUCTURED_INLINE_CASE) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Unknown unstructured-text variant case ${caseIndex}`,
      );
    }
    if (!payload || payload.kind !== 'text') {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Expected a 'text' payload for the unstructured-text inline case`,
      );
    }
    const language = payload.value.language;
    // Lenient decode (matches Rust/schema `check_text`): a missing language is
    // always allowed; only a present language outside the allow-list is rejected.
    if (allowedCodes.length > 0 && language !== undefined && !allowedCodes.includes(language)) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Language code \`${language}\` is not allowed. Allowed codes: ${allowedCodes.join(', ')}`,
      );
    }
    return {
      tag: 'inline',
      val: payload.value.text,
      languageCode: language,
    } as UnstructuredText<LC>;
  },

  /**
   * Encodes an `UnstructuredText` into a schema-native unstructured-text
   * `variant { inline, url }` value.
   */
  toSchemaValue<LC extends LanguageCode[]>(input: UnstructuredText<LC>): SchemaValue {
    if (input.tag === 'url') {
      return {
        kind: 'variant',
        value: {
          case: UNSTRUCTURED_URL_CASE,
          payload: { kind: 'url', value: { url: input.val } },
        },
      };
    }
    return {
      kind: 'variant',
      value: {
        case: UNSTRUCTURED_INLINE_CASE,
        payload: {
          kind: 'text',
          value: {
            text: input.val,
            language: input.languageCode as string | undefined,
          },
        },
      },
    };
  },

  /**
   * Creates `UnstructuredText` from a URL.
   *
   * ```ts
   * function foo(input: UnstructuredText) {..}
   *
   * foo(UnstructuredText.fromUrl("https://example.com/doc.txt"));
   * ```
   *
   * @param urlValue A URL string
   */
  fromUrl(urlValue: string): UnstructuredText {
    return {
      tag: 'url',
      val: urlValue,
    };
  },

  /**
   * Creates `UnstructuredText` from inline text data.
   *
   * ```ts
   * function foo(input: UnstructuredText<['en', 'de']>) {..}
   *
   * foo(UnstructuredText.fromInline("hello", 'en'));
   * ```
   *
   * If defining separately, please annotate the types to infer the types.
   *
   * ```ts
   *
   * const x: UnstructuredText<['en', 'de']> = UnstructuredText.fromInline("hello", 'en');
   *
   * foo(x);
   *
   * ```
   *
   * @param data
   * @param languageCode - The language code
   * @returns A `TextInput` object with `languageCode` set to `'en'`.
   */
  fromInline<LC extends LanguageCode[] = []>(
    data: string,
    languageCode?: LC[number],
  ): UnstructuredText<LC> {
    return {
      tag: 'inline',
      val: data,
      languageCode: languageCode,
    };
  },
};

/**
 * Represents inline unstructured binary input.
 *
 * Example usage:
 *
 * ```ts
 * const inlineBinary: UnstructuredBinary<'application/json'> =
 *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
 *```
 *
 * If no mime types are specified, any mime type is allowed. Note that
 * when using `inline` you always need to pass a mime-type as we don't allow
 * unstructured-binary without mime type.
 *
 * ```ts
 *  function foo(input: UnstructuredBinary) {..} // any mime type allowed
 *  function bar(input: UnstructuredBinary<['application/json', 'image/png']>) {..} // only application/json and image/png allowed
 *
 *  const imageBinary: UnstructuredBinary =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "image/jpeg");
 *
 *  const textBinary: UnstructuredBinary<'text/plain'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "text/plain");
 *
 *  foo(imageBinary); // allowed
 *  foo(textBinary); // allowed
 *
 *  bar(imageBinary); // not allowed
 *
 *  const appJsonBinary: UnstructuredBinary<'application/json'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "application/json");
 *
 *  bar(appJsonBinary); // allowed
 *
 * ```
 */
type MimeType = string;

export type UnstructuredBinary<MT extends MimeType[] | MimeType = MimeType> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: Uint8Array;
      mimeType: MT extends MimeType[] ? MT[number] : MimeType;
    };

export type UnstructuredBinaryType<MT extends MimeType[] | MimeType = MimeType> =
  UnstructuredBinary<MT>;

export const UnstructuredBinary = {
  /**
   * Decodes a schema-native unstructured-binary `variant { inline, url }` value
   * into an `UnstructuredBinary`, validating the mime type against
   * `allowedMimeTypes` when the agent declares a fixed set.
   */
  fromSchemaValue<MT extends string[] | MimeType = MimeType>(
    parameterName: string,
    value: SchemaValue,
    allowedMimeTypes: string[],
  ): UnstructuredBinary<MT> {
    if (value.kind !== 'variant') {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Expected an unstructured-binary 'variant' value, got '${value.kind}'`,
      );
    }
    const { case: caseIndex, payload } = value.value;
    if (caseIndex === UNSTRUCTURED_URL_CASE) {
      if (!payload || payload.kind !== 'url') {
        throw new Error(
          `Invalid value for parameter ${parameterName}. Expected a 'url' payload for the unstructured-binary url case`,
        );
      }
      return { tag: 'url', val: payload.value.url } as UnstructuredBinary<MT>;
    }
    if (caseIndex !== UNSTRUCTURED_INLINE_CASE) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Unknown unstructured-binary variant case ${caseIndex}`,
      );
    }
    if (!payload || payload.kind !== 'binary') {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Expected a 'binary' payload for the unstructured-binary inline case`,
      );
    }
    const mimeType = payload.value.mimeType;
    // Lenient decode (matches Rust/schema `check_binary`): a missing mime type
    // is always allowed; only a present mime type outside the allow-list is
    // rejected.
    if (
      allowedMimeTypes.length > 0 &&
      mimeType !== undefined &&
      !allowedMimeTypes.includes(mimeType)
    ) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Mime type \`${mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(', ')}`,
      );
    }
    return {
      tag: 'inline',
      val: new Uint8Array(payload.value.bytes),
      mimeType: mimeType ?? '',
    } as UnstructuredBinary<MT>;
  },

  /**
   * Encodes an `UnstructuredBinary` into a schema-native unstructured-binary
   * `variant { inline, url }` value.
   */
  toSchemaValue<MT extends MimeType[] | MimeType = MimeType>(
    input: UnstructuredBinary<MT>,
  ): SchemaValue {
    if (input.tag === 'url') {
      return {
        kind: 'variant',
        value: {
          case: UNSTRUCTURED_URL_CASE,
          payload: { kind: 'url', value: { url: input.val } },
        },
      };
    }
    return {
      kind: 'variant',
      value: {
        case: UNSTRUCTURED_INLINE_CASE,
        payload: {
          kind: 'binary',
          value: {
            bytes: Array.from(input.val),
            mimeType: input.mimeType as string | undefined,
          },
        },
      },
    };
  },

  /**
   * Creates a `UnstructuredBinary` from a URL.
   *
   * Example usage:
   *
   * ```ts
   *
   * const urlBinary: UnstructuredBinary =
   *   UnstructuredBinary.fromUrl("https://example.com/file.bin");
   *
   * ```
   *
   * @param urlValue
   */
  fromUrl(urlValue: string): UnstructuredBinary {
    return {
      tag: 'url',
      val: urlValue,
    };
  },

  /**
   * Creates a `UnstructuredBinary` from inline binary data.
   *
   * Example usage:
   *
   * ```ts
   *
   * const inlineBinary: UnstructuredBinary<'application/json'> =
   *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
   *
   * ```
   *
   * @param data
   * @param mimeType
   */
  fromInline<MT extends MimeType[] | MimeType = MimeType>(
    data: Uint8Array,
    mimeType: MT extends MimeType[] ? MT[number] : MimeType,
  ): UnstructuredBinary<MT> {
    return {
      tag: 'inline',
      val: data,
      mimeType: mimeType,
    };
  },
};

/** Encodes an optional value into a schema-native `option` value. */
export function encodeOption<T>(value: T | undefined, encode: (v: T) => SchemaValue): SchemaValue {
  if (value === undefined || value === null) {
    return { kind: 'option', value: {} };
  } else {
    return { kind: 'option', value: { inner: encode(value) } };
  }
}

/** Decodes a schema-native `option` value into an optional value. */
export function decodeOption<T>(value: SchemaValue, decode: (v: SchemaValue) => T): T | undefined {
  if (value.kind !== 'option') {
    throw new Error(`Expected option value, got '${value.kind}'`);
  }
  const inner = value.value.inner;
  if (inner === undefined || inner === null) {
    return undefined;
  } else {
    return decode(inner);
  }
}

/**
 * Encodes a record of booleans keyed by JS-cased field names into a
 * schema-native `flags` value. `flagPairs` lists `[wireName, jsName]` in the
 * schema's declaration order; the resulting `bits` array is positional.
 */
export function encodeFlags(
  value: Record<string, boolean>,
  flagPairs: [string, string][],
): SchemaValue {
  const bits = flagPairs.map(([, jsName]) => value[jsName] === true);
  return { kind: 'flags', value: { bits } };
}

/**
 * Decodes a schema-native `flags` value (a positional boolean `bits` array)
 * into a record of booleans keyed by the JS-cased field names.
 *
 * `initial` provides the exact result shape (every field initialised to
 * `false`) so the inferred return type stays precise. `flagPairs` lists
 * `[wireName, jsName]` in the schema's declaration order, aligned with `bits`.
 */
export function decodeFlags<T extends Record<string, boolean>>(
  value: SchemaValue,
  initial: T,
  flagPairs: [string, string][],
): T {
  if (value.kind !== 'flags') {
    throw new Error(`Expected flags value, got '${value.kind}'`);
  }
  const bits = value.value.bits;
  if (!Array.isArray(bits)) {
    throw new Error(`Expected boolean array for flags, got ${bits}`);
  }
  const result = { ...initial } as T;
  flagPairs.forEach(([, jsName], idx) => {
    (result as Record<string, boolean>)[jsName] = bits[idx] === true;
  });
  return result;
}

const STREAM_SUBPROTOCOL = 'golem.agent-invocation.v1';
const STREAM_ENDPOINT = '/v1/agents/invoke-agent-session';

export class StreamingProtocolError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'StreamingProtocolError';
  }
}

class RetryableStreamingConnectionError extends Error {}

export type PublicStreamReferencePolicy = 'none' | 'provisional' | 'stable';

/** Validates a public protocol value against an exact projected schema graph. */
export class PublicValueCodec {
  constructor(private readonly graph: SchemaGraph) {}

  validate(value: unknown, streamPolicy: PublicStreamReferencePolicy): PublicValue {
    new PublicValueValidator(this.graph, streamPolicy).validate(value);
    return value as PublicValue;
  }
}

export function publicValueCodec(graph: SchemaGraph): PublicValueCodec {
  return new PublicValueCodec(graph);
}

class PublicValueValidator {
  private charge = 0;
  private readonly streams = new Set<string>();

  constructor(
    private readonly graph: SchemaGraph,
    private readonly streamPolicy: PublicStreamReferencePolicy,
  ) {}

  validate(value: unknown): void {
    this.value(this.graph.root, value, 0);
  }

  private value(type: SchemaType, value: unknown, depth: number): void {
    if (depth >= 64) this.fail('resource-exhausted', 'schema value nesting exceeds 64 levels');
    this.add(1);
    const body = this.resolve(type);
    switch (body.tag) {
      case 'bool':
        if (typeof value !== 'boolean') this.mismatch('boolean');
        this.add(1);
        return;
      case 's8':
        this.integer(body, value, -128, 127, 1, 'signed');
        return;
      case 's16':
        this.integer(body, value, -32768, 32767, 2, 'signed');
        return;
      case 's32':
        this.integer(body, value, -2147483648, 2147483647, 4, 'signed');
        return;
      case 'u8':
        this.integer(body, value, 0, 255, 1, 'unsigned');
        return;
      case 'u16':
        this.integer(body, value, 0, 65535, 2, 'unsigned');
        return;
      case 'u32':
        this.integer(body, value, 0, 4294967295, 4, 'unsigned');
        return;
      case 's64':
        this.decimalInteger(body, value, true);
        return;
      case 'u64':
        this.decimalInteger(body, value, false);
        return;
      case 'f32':
        this.float(body, value, true);
        return;
      case 'f64':
        this.float(body, value, false);
        return;
      case 'char': {
        if (typeof value !== 'string' || [...value].length !== 1) this.mismatch('Unicode scalar');
        const point = value.codePointAt(0) as number;
        if (point >= 0xd800 && point <= 0xdfff) this.mismatch('Unicode scalar');
        this.string(value);
        return;
      }
      case 'string':
        if (typeof value !== 'string') this.mismatch('string');
        this.string(value);
        return;
      case 'record': {
        const input = publicObject(value, 'record');
        const expected = new Set(body.fields.map((field) => field.name));
        publicExactMembers(input, expected, 'record');
        this.collection(body.fields.length);
        for (const field of body.fields) {
          this.string(field.name);
          this.value(field.body, input[field.name], depth + 1);
        }
        return;
      }
      case 'variant': {
        const input = publicObject(value, 'variant');
        const name = publicString(input.$case, 'variant case');
        const selected = body.cases.find((item) => item.name === name);
        if (!selected) this.fail('validation-error', `unknown variant case '${name}'`);
        publicExactMembers(
          input,
          new Set(selected.payload ? ['$case', 'value'] : ['$case']),
          'variant',
        );
        this.string(name);
        if (selected.payload) this.value(selected.payload, input.value, depth + 1);
        return;
      }
      case 'enum': {
        const name = publicString(value, 'enum');
        if (!body.cases.includes(name))
          this.fail('validation-error', `unknown enum case '${name}'`);
        this.string(name);
        return;
      }
      case 'flags': {
        const values = publicArray(value, 'flags');
        this.collection(values.length);
        let previous = -1;
        for (const item of values) {
          const name = publicString(item, 'flag');
          const index = body.names.indexOf(name);
          if (index <= previous)
            this.fail('validation-error', 'flags must be unique and in declaration order');
          previous = index;
          this.string(name);
        }
        return;
      }
      case 'tuple': {
        const values = publicArray(value, 'tuple');
        if (values.length !== body.elements.length)
          this.fail('validation-error', 'tuple arity does not match schema');
        this.collection(values.length);
        body.elements.forEach((element, index) => this.value(element, values[index], depth + 1));
        return;
      }
      case 'list':
        this.repeated(body.element, value, depth, undefined);
        return;
      case 'fixed-list':
        this.repeated(body.element, value, depth, body.length);
        return;
      case 'map': {
        const entries = publicArray(value, 'map');
        this.collection(entries.length);
        for (const entry of entries) {
          const pair = publicArray(entry, 'map entry');
          if (pair.length !== 2)
            this.fail('validation-error', 'map entry must contain exactly two elements');
          this.value(body.key, pair[0], depth + 1);
          this.value(body.value, pair[1], depth + 1);
        }
        return;
      }
      case 'option': {
        const input = publicObject(value, 'option');
        const tag = publicString(input.$option, 'option tag');
        if (tag === 'none') publicExactMembers(input, new Set(['$option']), 'option');
        else if (tag === 'some') {
          publicExactMembers(input, new Set(['$option', 'value']), 'option');
          this.value(body.element, input.value, depth + 1);
        } else this.fail('validation-error', `invalid option tag '${tag}'`);
        this.string(tag);
        return;
      }
      case 'result': {
        const input = publicObject(value, 'result');
        const tag = publicString(input.$result, 'result tag');
        const payload = tag === 'ok' ? body.ok : tag === 'err' ? body.err : undefined;
        if (tag !== 'ok' && tag !== 'err')
          this.fail('validation-error', `invalid result tag '${tag}'`);
        publicExactMembers(input, new Set(payload ? ['$result', 'value'] : ['$result']), 'result');
        this.string(tag);
        if (payload) this.value(payload, input.value, depth + 1);
        return;
      }
      case 'text':
        this.text(body.restrictions, value);
        return;
      case 'binary':
        this.binary(body.restrictions, value);
        return;
      case 'path': {
        const path = publicString(value, 'path');
        if (path.length === 0) this.fail('validation-error', 'path must be non-empty');
        const parts = path.split('/');
        const name = parts[parts.length - 1] ?? path;
        const index = name.lastIndexOf('.');
        if (index >= 0 && index < name.length - 1 && body.spec.allowedExtensions) {
          const extension = name.slice(index + 1);
          if (!body.spec.allowedExtensions.includes(extension))
            this.fail('validation-error', 'path extension is not allowed');
        }
        this.string(path);
        return;
      }
      case 'url':
        this.url(body.restrictions, value);
        return;
      case 'datetime': {
        const datetime = publicString(value, 'datetime');
        if (!validPublicDatetime(datetime))
          this.fail('validation-error', 'datetime must be canonical RFC 3339 UTC');
        this.string(datetime);
        return;
      }
      case 'duration': {
        const input = publicObject(value, 'duration');
        publicExactMembers(input, new Set(['nanoseconds']), 'duration');
        publicDecimal(input.nanoseconds, true, -(1n << 63n), (1n << 63n) - 1n);
        this.add(8);
        return;
      }
      case 'quantity':
        this.quantity(body.spec, value);
        return;
      case 'union': {
        const input = publicObject(value, 'union');
        publicExactMembers(input, new Set(['$union', 'value']), 'union');
        const tag = publicString(input.$union, 'union tag');
        const branch = body.branches.find((item) => item.tag === tag);
        if (!branch) this.fail('validation-error', `unknown union branch '${tag}'`);
        this.value(branch.body, input.value, depth + 1);
        if (!publicDiscriminatorMatches(branch.discriminator, input.value))
          this.fail('validation-error', 'union body does not satisfy its discriminator');
        this.string(tag);
        return;
      }
      case 'stream':
        this.stream(value);
        return;
      case 'secret':
      case 'quota-token':
      case 'permission-card':
      case 'future':
        this.fail(
          'unsupported-value',
          `schema type '${body.tag}' cannot cross the public boundary`,
        );
        return;
      case 'ref':
        this.fail('validation-error', 'unresolved schema reference');
    }
  }

  private resolve(type: SchemaType): SchemaTypeBody {
    let current = type;
    const seen = new Set<string>();
    while (current.body.tag === 'ref') {
      const id = current.body.id;
      if (seen.has(id)) this.fail('validation-error', `cyclic schema alias '${id}'`);
      seen.add(id);
      const definition = this.graph.defs.get(id);
      if (!definition) this.fail('validation-error', `unresolved schema reference '${id}'`);
      current = definition.body;
    }
    return current.body;
  }

  private repeated(type: SchemaType, value: unknown, depth: number, length?: number): void {
    const values = publicArray(value, length === undefined ? 'list' : 'fixed-list');
    if (length !== undefined && values.length !== length)
      this.fail('validation-error', 'fixed-list length does not match schema');
    this.collection(values.length);
    values.forEach((item) => this.value(type, item, depth + 1));
  }

  private integer(
    body: { restrictions?: NumericRestrictions },
    value: unknown,
    min: number,
    max: number,
    width: number,
    family: 'signed' | 'unsigned',
  ): void {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < min || value > max)
      this.fail('validation-error', 'integer is out of range');
    this.numeric(body.restrictions, family, BigInt(value));
    this.add(width);
  }

  private decimalInteger(
    body: { restrictions?: NumericRestrictions },
    value: unknown,
    signed: boolean,
  ): void {
    const parsed = publicDecimal(
      value,
      signed,
      signed ? -(1n << 63n) : 0n,
      signed ? (1n << 63n) - 1n : (1n << 64n) - 1n,
    );
    this.numeric(body.restrictions, signed ? 'signed' : 'unsigned', parsed);
    this.add(8);
  }

  private float(body: { restrictions?: NumericRestrictions }, value: unknown, f32: boolean): void {
    let number: number;
    if (typeof value === 'number') {
      number = value;
      if (!Number.isFinite(number)) this.mismatch('finite JSON number');
      if (f32) {
        number = Math.fround(number);
        if (!Number.isFinite(number)) this.fail('validation-error', 'f32 is out of range');
      }
    } else {
      const input = publicObject(value, 'exceptional float');
      publicExactMembers(input, new Set(['$float']), 'exceptional float');
      const tag = publicString(input.$float, 'exceptional float tag');
      number =
        tag === 'nan'
          ? Number.NaN
          : tag === 'positive-infinity'
            ? Number.POSITIVE_INFINITY
            : tag === 'negative-infinity'
              ? Number.NEGATIVE_INFINITY
              : this.fail('validation-error', `invalid exceptional float tag '${tag}'`);
    }
    if (Number.isFinite(number)) this.numeric(body.restrictions, 'float', number);
    else if (body.restrictions?.min || body.restrictions?.max)
      this.fail('validation-error', 'exceptional float does not satisfy numeric restrictions');
    this.add(f32 ? 4 : 8);
  }

  private numeric(
    restrictions: NumericRestrictions | undefined,
    family: 'signed' | 'unsigned' | 'float',
    value: bigint | number,
  ): void {
    if (!restrictions) return;
    const compare = (bound: NumericBound): number => {
      if (family === 'float') {
        if (bound.tag !== 'float-bits')
          this.fail('validation-error', 'numeric bound family does not match schema');
        const bytes = Buffer.allocUnsafe(8);
        bytes.writeBigUInt64BE(BigInt.asUintN(64, bound.val));
        const decoded = bytes.readDoubleBE();
        return (value as number) < decoded ? -1 : (value as number) > decoded ? 1 : 0;
      }
      if (bound.tag !== family)
        this.fail('validation-error', 'numeric bound family does not match schema');
      return (value as bigint) < bound.val ? -1 : (value as bigint) > bound.val ? 1 : 0;
    };
    if (restrictions.min && compare(restrictions.min) < 0)
      this.fail('validation-error', 'number is below schema minimum');
    if (restrictions.max && compare(restrictions.max) > 0)
      this.fail('validation-error', 'number is above schema maximum');
  }

  private text(restrictions: TextRestrictions, value: unknown): void {
    const input = publicObject(value, 'text');
    publicExactOptionalMembers(input, new Set(['text']), new Set(['language']), 'text');
    const text = publicString(input.text, 'text body');
    const language =
      input.language === undefined ? undefined : publicString(input.language, 'language');
    if (language !== undefined && !/^[A-Za-z]{1,8}(?:-[A-Za-z0-9]{1,8})*$/u.test(language))
      this.fail('validation-error', 'invalid BCP-47 language tag');
    if (
      language !== undefined &&
      restrictions.languages &&
      restrictions.languages.length > 0 &&
      !restrictions.languages.includes(language)
    )
      this.fail('validation-error', 'text language is not allowed');
    const length = [...text].length;
    if (restrictions.minLength !== undefined && length < restrictions.minLength)
      this.fail('validation-error', 'text is shorter than schema minimum');
    if (restrictions.maxLength !== undefined && length > restrictions.maxLength)
      this.fail('validation-error', 'text is longer than schema maximum');
    if (restrictions.regex !== undefined && !new RegExp(restrictions.regex, 'u').test(text))
      this.fail('validation-error', 'text does not match schema regex');
    this.string(text);
    if (language !== undefined) this.string(language);
  }

  private binary(restrictions: BinaryRestrictions, value: unknown): void {
    const input = publicObject(value, 'binary');
    publicExactOptionalMembers(input, new Set(['bytes']), new Set(['mimeType']), 'binary');
    const encoded = publicString(input.bytes, 'binary bytes');
    if (
      encoded.length % 4 !== 0 ||
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(encoded)
    )
      this.fail('malformed-message', 'binary bytes are not canonical padded base64');
    const bytes = Buffer.from(encoded, 'base64');
    if (bytes.toString('base64') !== encoded)
      this.fail('malformed-message', 'binary bytes are not canonical padded base64');
    const mime =
      input.mimeType === undefined ? undefined : publicString(input.mimeType, 'MIME type');
    if (mime !== undefined && !/^[A-Za-z0-9!#$&^_.+\-]+\/[A-Za-z0-9!#$&^_.+\-]+$/u.test(mime))
      this.fail('validation-error', 'invalid MIME type');
    if (
      mime !== undefined &&
      restrictions.mimeTypes &&
      restrictions.mimeTypes.length > 0 &&
      !restrictions.mimeTypes.includes(mime)
    )
      this.fail('validation-error', 'binary MIME type is not allowed');
    if (restrictions.minBytes !== undefined && bytes.length < restrictions.minBytes)
      this.fail('validation-error', 'binary is shorter than schema minimum');
    if (restrictions.maxBytes !== undefined && bytes.length > restrictions.maxBytes)
      this.fail('validation-error', 'binary is longer than schema maximum');
    this.add(bytes.length);
    if (mime !== undefined) this.string(mime);
  }

  private url(restrictions: UrlRestrictions, value: unknown): void {
    const raw = publicString(value, 'URL');
    if (raw.length === 0 || /[\u0000-\u0020\u007f]/u.test(raw))
      this.fail('validation-error', 'invalid URL');
    let parsed: URL;
    try {
      parsed = new URL(raw);
    } catch {
      this.fail('validation-error', 'invalid URL');
    }
    const scheme = parsed.protocol.slice(0, -1);
    if (restrictions.allowedSchemes?.every((item) => item.toLowerCase() !== scheme.toLowerCase()))
      this.fail('validation-error', 'URL scheme is not allowed');
    if (
      restrictions.allowedHosts &&
      (parsed.hostname.length === 0 ||
        restrictions.allowedHosts.every(
          (item) => item.toLowerCase() !== parsed.hostname.toLowerCase(),
        ))
    )
      this.fail('validation-error', 'URL host is not allowed');
    this.string(raw);
  }

  private quantity(spec: QuantitySpec, value: unknown): void {
    const input = publicObject(value, 'quantity');
    publicExactMembers(input, new Set(['mantissa', 'scale', 'unit']), 'quantity');
    const mantissa = publicDecimal(input.mantissa, true, -(1n << 63n), (1n << 63n) - 1n);
    if (
      typeof input.scale !== 'number' ||
      !Number.isInteger(input.scale) ||
      input.scale < -2147483648 ||
      input.scale > 2147483647
    )
      this.fail('validation-error', 'quantity scale is out of range');
    const unit = publicString(input.unit, 'quantity unit');
    const allowed = spec.allowedSuffixes.length === 0 ? [spec.baseUnit] : spec.allowedSuffixes;
    if (!allowed.includes(unit)) this.fail('validation-error', 'quantity unit is not allowed');
    const current: QuantityValue = { mantissa, scale: input.scale, unit };
    if (spec.min && !publicQuantityLe(spec.min, current))
      this.fail('validation-error', 'quantity is below schema minimum');
    if (spec.max && !publicQuantityLe(current, spec.max))
      this.fail('validation-error', 'quantity is above schema maximum');
    this.add(12);
    this.string(unit);
  }

  private stream(value: unknown): void {
    if (this.streamPolicy === 'none')
      this.fail('unsupported-value', 'stream references are not allowed in this value');
    const outer = publicObject(value, 'stream reference');
    publicExactMembers(outer, new Set(['$stream']), 'stream reference');
    const identity = publicObject(outer.$stream, 'stream identity');
    const expected = this.streamPolicy === 'provisional' ? 'provisionalRef' : 'streamToken';
    publicExactMembers(identity, new Set([expected]), 'stream identity');
    const reference = publicString(identity[expected], 'stream identity');
    if (this.streams.has(`${expected}:${reference}`))
      this.fail('stream-already-consumed', 'stream reference appears more than once');
    this.streams.add(`${expected}:${reference}`);
    if (expected === 'provisionalRef') {
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(reference))
        this.fail('validation-error', 'provisional stream reference must be a lower-case UUIDv4');
      this.add(16);
    } else {
      if (
        reference.length === 0 ||
        Buffer.byteLength(reference) > 8192 ||
        [...reference].some((character) => character.codePointAt(0)! > 0x7f)
      )
        this.fail('token-invalid', 'invalid stream token');
      this.string(reference);
    }
  }

  private collection(size: number): void {
    if (size > 100000) this.fail('resource-exhausted', 'collection exceeds 100000 entries');
    this.add(4);
  }

  private string(value: string): void {
    if (!isUnicodeScalarString(value)) this.fail('validation-error', 'invalid Unicode string');
    this.add(Buffer.byteLength(value));
  }

  private add(amount: number): void {
    this.charge += amount;
    if (this.charge > 16 * 1024 * 1024)
      this.fail('resource-exhausted', 'schema value exceeds 16 MiB');
  }

  private mismatch(expected: string): never {
    return this.fail('validation-error', `expected ${expected}`);
  }

  private fail(code: string, message: string): never {
    throw new StreamingProtocolError(code, `Public value codec: ${message}`);
  }
}

function publicObject(value: unknown, what: string): Record<string, unknown> {
  if (!isRecord(value))
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: expected ${what} object`,
    );
  return value;
}

function publicArray(value: unknown, what: string): unknown[] {
  if (!Array.isArray(value))
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: expected ${what} array`,
    );
  return value;
}

function publicString(value: unknown, what: string): string {
  if (typeof value !== 'string')
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: expected ${what} string`,
    );
  if (!isUnicodeScalarString(value))
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: invalid Unicode in ${what}`,
    );
  return value;
}

function publicExactMembers(
  value: Record<string, unknown>,
  expected: Set<string>,
  what: string,
): void {
  const actual = Object.keys(value);
  if (actual.length !== expected.size || actual.some((name) => !expected.has(name)))
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: invalid members in ${what}`,
    );
}

function publicExactOptionalMembers(
  value: Record<string, unknown>,
  required: Set<string>,
  optional: Set<string>,
  what: string,
): void {
  const actual = new Set(Object.keys(value));
  if (
    [...required].some((name) => !actual.has(name)) ||
    [...actual].some((name) => !required.has(name) && !optional.has(name))
  )
    throw new StreamingProtocolError(
      'validation-error',
      `Public value codec: invalid members in ${what}`,
    );
}

function publicDecimal(value: unknown, signed: boolean, min: bigint, max: bigint): bigint {
  if (
    typeof value !== 'string' ||
    !(signed ? /^(?:0|-[1-9]\d*|[1-9]\d*)$/u : /^(?:0|[1-9]\d*)$/u).test(value)
  )
    throw new StreamingProtocolError(
      'validation-error',
      'Public value codec: non-canonical decimal string',
    );
  const parsed = BigInt(value);
  if (parsed < min || parsed > max)
    throw new StreamingProtocolError(
      'validation-error',
      'Public value codec: decimal integer is out of range',
    );
  return parsed;
}

function publicDiscriminatorMatches(rule: DiscriminatorRule, value: unknown): boolean {
  const string =
    typeof value === 'string'
      ? value
      : isRecord(value) && typeof value.text === 'string'
        ? value.text
        : undefined;
  switch (rule.tag) {
    case 'prefix':
      return string?.startsWith(rule.val) === true;
    case 'suffix':
      return string?.endsWith(rule.val) === true;
    case 'contains':
      return string?.includes(rule.val) === true;
    case 'regex':
      return string !== undefined && new RegExp(rule.val, 'u').test(string);
    case 'field-equals':
      return (
        isRecord(value) &&
        Object.prototype.hasOwnProperty.call(value, rule.val.fieldName) &&
        (rule.val.literal === undefined ||
          publicDiscriminatorString(value[rule.val.fieldName]) === rule.val.literal)
      );
    case 'field-absent':
      return isRecord(value) && !Object.prototype.hasOwnProperty.call(value, rule.val);
  }
}

function publicDiscriminatorString(value: unknown): string | undefined {
  return typeof value === 'string'
    ? value
    : isRecord(value) && typeof value.text === 'string'
      ? value.text
      : undefined;
}

function publicQuantityLe(left: QuantityValue, right: QuantityValue): boolean {
  const common = Math.max(left.scale, right.scale);
  const leftShift = common - left.scale;
  const rightShift = common - right.scale;
  if (leftShift > 38 || rightShift > 38)
    throw new StreamingProtocolError(
      'validation-error',
      'Public value codec: quantity comparison overflows',
    );
  const leftValue = left.mantissa * 10n ** BigInt(leftShift);
  const rightValue = right.mantissa * 10n ** BigInt(rightShift);
  const min = -(1n << 127n);
  const max = (1n << 127n) - 1n;
  if (leftValue < min || leftValue > max || rightValue < min || rightValue > max)
    throw new StreamingProtocolError(
      'validation-error',
      'Public value codec: quantity comparison overflows',
    );
  return leftValue <= rightValue;
}

function validPublicDatetime(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?Z$/u.exec(value);
  if (!match) return false;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  if (month < 1 || month > 12 || hour > 23 || minute > 59 || second > 59) return false;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return day >= 1 && day <= days[month - 1];
}

type OutputResolver = (
  token: string,
  decode: (value: unknown) => unknown,
  schemaIdentity?: string,
  wireKind?: 'json' | 'u8' | 'binary',
) => AgentStream<unknown>;
type InputRegistrar = (
  source: AsyncIterable<unknown>,
  encode: (value: unknown) => unknown,
  wireKind?: 'json' | 'u8' | 'binary',
) => unknown;

/**
 * Creates the Node-only public v1 WebSocket implementation used by generated
 * stream-bearing methods. Stream-free methods continue to use the REST runtime.
 */
export function createStreamingRemoteMethod<Args extends any[], R>(
  getServer: () => GolemServer,
  getDescriptor: () => StreamingInvocationDescriptor,
  encode: (args: Args, stream: InputRegistrar) => PublicValue,
  decode: (value: unknown, stream: OutputResolver) => R,
): StreamingRemoteMethod<Args, R> {
  const invoke = (signal: AbortSignal | undefined, args: Args) =>
    new StreamingSession(getServer(), getDescriptor(), signal).start(
      (register) => encode(args, register),
      (value, resolve) => decode(value, resolve),
    );
  const method = ((...args: Args) => invoke(undefined, args)) as StreamingRemoteMethod<Args, R>;
  method.abortable = (signal: AbortSignal, ...args: Args) => invoke(signal, args);
  return method;
}

type InputState = {
  source: AsyncIterator<unknown>;
  encode: (value: unknown) => unknown;
  channel?: number;
  sequence: bigint;
  terminal: boolean;
  cancelReason?: 'source-unavailable';
  wireKind: 'json' | 'u8' | 'binary';
  bufferedPull?: Promise<IteratorResult<unknown>>;
  naturalEnd?: boolean;
  pending?: PendingInput;
};

type PendingInput = {
  sequence: bigint;
  itemCount: bigint;
  terminal: boolean;
  bytes: number;
  render: (channel: number) => string | Buffer;
  trim?: (accepted: bigint) => PendingInput;
};

const MAX_QUEUE_ITEMS = 256;
const MAX_QUEUE_BYTES = 16 * 1024 * 1024;
const MAX_SESSION_QUEUE_BYTES = 32 * 1024 * 1024;
const MAX_LOGICAL_VALUE_BYTES = 16 * 1024 * 1024;
const U64_MAX = 18_446_744_073_709_551_615n;

function validateOpaqueToken(value: unknown, name: string): asserts value is string {
  if (
    typeof value !== 'string' ||
    value.length === 0 ||
    value.length > 8192 ||
    !/^[\x20-\x7e]+$/u.test(value)
  )
    throw new StreamingProtocolError(
      name === 'cursor token' ? 'invalid-cursor' : 'token-invalid',
      `${name} must be 1..8192 printable ASCII bytes`,
    );
}

function validateChannel(value: unknown): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < 1 || (value as number) > 0xffff_ffff)
    throw new StreamingProtocolError('invalid-channel', 'channel must be a u32 integer');
}

function canonicalJson(value: unknown, depth = 0): string {
  if (depth >= 64)
    throw new StreamingProtocolError('resource-exhausted', 'JSON nesting exceeds 64 levels');
  if (value === null) return 'null';
  if (typeof value === 'boolean') return JSON.stringify(value);
  if (typeof value === 'string') {
    if (!isUnicodeScalarString(value))
      throw new StreamingProtocolError('validation-error', 'invalid Unicode string');
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value))
      throw new StreamingProtocolError('validation-error', 'non-finite JSON number');
    return Object.is(value, -0) ? '-0' : JSON.stringify(value);
  }
  if (Array.isArray(value))
    return `[${value.map((item) => canonicalJson(item, depth + 1)).join(',')}]`;
  if (isRecord(value)) {
    const entries = Object.keys(value)
      .map((key) => {
        if (!isUnicodeScalarString(key))
          throw new StreamingProtocolError('validation-error', 'invalid Unicode object key');
        return key;
      })
      .sort(compareCodePoints)
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key], depth + 1)}`);
    return `{${entries.join(',')}}`;
  }
  throw new StreamingProtocolError('validation-error', 'value is not canonical JSON');
}

/** Runtime conformance hook for the public v1 binary envelope codec. */
export function encodeStreamSessionBinaryEnvelope(metadata: unknown, payload: Uint8Array): Buffer {
  const json = canonicalJson(metadata);
  const prefix = Buffer.allocUnsafe(4);
  prefix.writeUInt32BE(Buffer.byteLength(json));
  return Buffer.concat([prefix, Buffer.from(json), Buffer.from(payload)]);
}

/** Runtime conformance hook for canonical public v1 text frames. */
export function encodeStreamSessionTextFrame(value: unknown): string {
  return canonicalJson(value);
}

/** Runtime conformance hook for strict public v1 JSON parsing. */
export function parseStreamSessionTextFrame(text: string): unknown {
  return parseStrictJson(text);
}

function isUnicodeScalarString(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      index += 1;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return false;
    }
  }
  return true;
}

function compareCodePoints(left: string, right: string): number {
  const leftPoints = [...left].map((value) => value.codePointAt(0) as number);
  const rightPoints = [...right].map((value) => value.codePointAt(0) as number);
  for (let index = 0; index < Math.min(leftPoints.length, rightPoints.length); index += 1) {
    if (leftPoints[index] !== rightPoints[index]) return leftPoints[index] - rightPoints[index];
  }
  return leftPoints.length - rightPoints.length;
}

function parseStrictJson(text: string): unknown {
  let offset = 0;
  let collectionItems = 0;
  const whitespace = () => {
    while (/[ \t\r\n]/u.test(text[offset] ?? '')) offset += 1;
  };
  const string = (): string => {
    const start = offset;
    offset += 1;
    while (offset < text.length) {
      if (text[offset] === '\\') offset += 2;
      else if (text[offset++] === '"') {
        const token = text.slice(start, offset);
        const value = JSON.parse(token) as unknown;
        if (typeof value !== 'string' || !isUnicodeScalarString(value)) throw new Error();
        return value;
      } else if (text.charCodeAt(offset - 1) < 0x20) throw new Error();
    }
    throw new Error();
  };
  const value = (depth = 0): unknown => {
    if (depth >= 64) throw new Error();
    whitespace();
    const char = text[offset];
    if (char === '"') return string();
    if (char === '[') {
      const result: unknown[] = [];
      offset += 1;
      whitespace();
      if (text[offset] === ']') return ((offset += 1), result);
      for (;;) {
        if ((collectionItems += 1) > 100_000) throw new Error();
        result.push(value(depth + 1));
        whitespace();
        if (text[offset++] === ']') return result;
        if (text[offset - 1] !== ',') throw new Error();
      }
    }
    if (char === '{') {
      const result: Record<string, unknown> = {};
      const keys = new Set<string>();
      offset += 1;
      whitespace();
      if (text[offset] === '}') return ((offset += 1), result);
      for (;;) {
        whitespace();
        if (text[offset] !== '"') throw new Error();
        const key = string();
        if (keys.has(key)) throw new Error();
        keys.add(key);
        if ((collectionItems += 1) > 100_000) throw new Error();
        whitespace();
        if (text[offset++] !== ':') throw new Error();
        result[key] = value(depth + 1);
        whitespace();
        if (text[offset++] === '}') return result;
        if (text[offset - 1] !== ',') throw new Error();
      }
    }
    const match = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/u.exec(
      text.slice(offset),
    );
    if (!match) throw new Error();
    offset += match[0].length;
    return JSON.parse(match[0]);
  };
  try {
    const result = value();
    whitespace();
    if (offset !== text.length) throw new Error();
    return result;
  } catch {
    throw new StreamingProtocolError('malformed-message', 'invalid JSON');
  }
}

function object(value: unknown, required: string[], optional: string[] = []): Record<string, any> {
  if (!isRecord(value)) throw new StreamingProtocolError('malformed-message', 'expected object');
  const allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !(key in value)) ||
    Object.keys(value).some((key) => !allowed.has(key))
  )
    throw new StreamingProtocolError('malformed-message', 'invalid message fields');
  return value;
}

function decimal(value: unknown, field: string): bigint {
  if (typeof value !== 'string' || !/^(?:0|[1-9]\d*)$/u.test(value))
    throw new StreamingProtocolError(
      'malformed-message',
      `${field} must be a canonical unsigned decimal string`,
    );
  const result = BigInt(value);
  if (result > U64_MAX)
    throw new StreamingProtocolError('invalid-sequence', `${field} exceeds u64`);
  return result;
}

type OutputEntry = {
  value?: unknown;
  cursor?: { token: string; sequence: bigint };
  deliveredSequence?: bigint;
  error?: unknown;
  done?: true;
  packed?: { payload: Uint8Array; index: number };
  bytes: number;
};

class OutputStream<T> implements AgentStream<T> {
  private claimed = false;
  private waiting: Array<{
    resolve: (value: IteratorResult<T>) => void;
    reject: (error: unknown) => void;
  }> = [];
  private queue: Array<OutputEntry> = [];
  private queueBytes = 0;
  private protocolTerminal = false;
  private deliveredTerminal = false;
  private consumerDropped = false;
  private exposed = false;
  private receivedSequence = 0n;
  private deliveredSequence = 0n;
  private wireKind?: 'json' | 'u8' | 'binary';
  private decode?: (value: unknown) => T;

  constructor(
    private readonly dropped: () => void,
    private readonly delivered: (
      cursor: { token: string; sequence: bigint },
      deliveredSequence: bigint,
    ) => void,
    private readonly queued: (delta: number) => void,
    private readonly deliveryFailed: (error: unknown) => void,
  ) {}

  expose(wireKind: 'json' | 'u8' | 'binary'): void {
    if (this.exposed) throw new StreamingProtocolError('stream-conflict', 'output stream reused');
    if (this.wireKind && this.wireKind !== wireKind)
      throw new StreamingProtocolError('stream-conflict', 'output stream lane changed');
    this.exposed = true;
    this.wireKind = wireKind;
  }

  setDecoder(decode: (value: unknown) => T): void {
    this.decode = decode;
  }

  accept(first: bigint, count: bigint, wireKind?: 'json' | 'u8' | 'binary'): bigint {
    if (this.protocolTerminal)
      throw new StreamingProtocolError(
        'protocol-error',
        'message received after output stream terminal',
      );
    if (wireKind && this.wireKind && this.wireKind !== wireKind)
      throw new StreamingProtocolError('stream-conflict', 'output stream lane changed');
    if (wireKind) this.wireKind = wireKind;
    if (first !== this.receivedSequence)
      throw new StreamingProtocolError('invalid-sequence', 'output sequence is not contiguous');
    if (count > U64_MAX - first)
      throw new StreamingProtocolError('invalid-sequence', 'output sequence overflow');
    this.receivedSequence = first + count;
    return this.receivedSequence;
  }

  prepareResume(): void {
    this.queued(-this.queueBytes);
    this.queue = [];
    this.queueBytes = 0;
    this.protocolTerminal = this.deliveredTerminal;
    this.receivedSequence = this.deliveredSequence;
  }

  isTerminal(): boolean {
    return this.protocolTerminal;
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    if (this.claimed) {
      throw new Error('AgentStream can only be iterated once');
    }
    this.claimed = true;
    return {
      next: () => this.next(),
      return: async () => {
        if (!this.protocolTerminal && !this.consumerDropped) this.dropped();
        this.consumerDropped = true;
        this.queued(-this.queueBytes);
        this.queue = [];
        this.queueBytes = 0;
        for (const waiting of this.waiting) waiting.resolve({ done: true, value: undefined });
        this.waiting = [];
        return { done: true, value: undefined };
      },
    };
  }

  push(
    value: unknown,
    cursor: { token: string; sequence: bigint } | undefined,
    bytes: number,
    deliveredSequence?: bigint,
  ): void {
    this.enqueue({ value, cursor, bytes, deliveredSequence });
  }

  pushPacked(
    payload: Uint8Array,
    cursor: { token: string; sequence: bigint },
    deliveredSequence: bigint,
  ): void {
    this.enqueue({
      packed: { payload, index: 0 },
      cursor,
      deliveredSequence,
      bytes: payload.byteLength,
    });
  }

  finish(cursor?: { token: string; sequence: bigint }, deliveredSequence?: bigint): void {
    this.protocolTerminal = true;
    if (!this.consumerDropped) this.enqueue({ done: true, cursor, deliveredSequence, bytes: 1 });
  }

  fail(
    error: unknown,
    cursor?: { token: string; sequence: bigint },
    deliveredSequence?: bigint,
  ): void {
    if (this.protocolTerminal) return;
    this.protocolTerminal = true;
    if (!this.consumerDropped) this.enqueue({ error, cursor, deliveredSequence, bytes: 1 });
  }

  abort(error: unknown): void {
    this.protocolTerminal = true;
    this.queued(-this.queueBytes);
    this.queue = [];
    this.queueBytes = 0;
    if (this.waiting.length > 0) {
      for (const waiting of this.waiting) waiting.reject(error);
      this.waiting = [];
    } else if (!this.consumerDropped) this.queue.push({ error, bytes: 0 });
  }

  private enqueue(item: OutputEntry): void {
    if (this.consumerDropped) return;
    if (this.queue.length >= MAX_QUEUE_ITEMS || this.queueBytes + item.bytes > MAX_QUEUE_BYTES) {
      throw new StreamingProtocolError(
        'resource-exhausted',
        'output delivery queue limit exceeded',
      );
    }
    this.queued(item.bytes);
    this.queue.push(item);
    this.queueBytes += item.bytes;
    while (this.waiting.length > 0) {
      const waiting = this.waiting[0];
      const next = this.take();
      if (!next) break;
      this.waiting.shift();
      this.deliver(next, waiting.resolve, waiting.reject);
    }
    if (this.protocolTerminal && this.queue.length === 0) {
      for (const waiting of this.waiting) waiting.resolve({ done: true, value: undefined });
      this.waiting = [];
    }
  }

  private next(): Promise<IteratorResult<T>> {
    if (this.consumerDropped) return Promise.resolve({ done: true, value: undefined });
    const item = this.take();
    if (item) return new Promise((resolve, reject) => this.deliver(item, resolve, reject));
    if (this.protocolTerminal) return Promise.resolve({ done: true, value: undefined });
    return new Promise((resolve, reject) => this.waiting.push({ resolve, reject }));
  }

  private take(): OutputEntry | undefined {
    const item = this.queue[0];
    if (!item) return undefined;
    if (item.packed) {
      const index = item.packed.index;
      const final = index === item.packed.payload.length - 1;
      const value = item.packed.payload[index];
      item.packed.index += 1;
      if (final) {
        this.queue.shift();
        this.queueBytes -= item.bytes;
        this.queued(-item.bytes);
      }
      return {
        value,
        cursor: final ? item.cursor : undefined,
        deliveredSequence: final ? item.deliveredSequence : undefined,
        bytes: 0,
      };
    }
    this.queue.shift();
    this.queueBytes -= item.bytes;
    this.queued(-item.bytes);
    return item;
  }

  private deliver(
    item: OutputEntry,
    resolve: (value: IteratorResult<T>) => void,
    reject: (error: unknown) => void,
  ): void {
    let result: IteratorResult<T>;
    try {
      if (item.error !== undefined) {
        if (item.cursor) this.checkpoint(item);
        reject(item.error);
        return;
      } else if (item.done) result = { done: true, value: undefined };
      else {
        if (!this.decode)
          throw new StreamingProtocolError(
            'protocol-error',
            'output was consumed before its stream reference',
          );
        result = { done: false, value: this.decode(item.value) };
      }
      if (item.cursor) this.checkpoint(item);
      resolve(result);
    } catch (error) {
      reject(error);
      this.deliveryFailed(error);
    }
  }

  private checkpoint(item: OutputEntry): void {
    if (!item.cursor) return;
    if (item.deliveredSequence === undefined)
      throw new StreamingProtocolError('protocol-error', 'cursor has no delivery sequence');
    this.deliveredSequence = item.deliveredSequence;
    this.delivered(item.cursor, item.deliveredSequence);
    if (item.done || item.error !== undefined) this.deliveredTerminal = true;
  }
}

class StreamingSession {
  private socket?: WebSocket;
  private accepted = false;
  private everAccepted = false;
  private pendingResume = false;
  private sessionToken?: string;
  private attemptId = crypto.randomUUID();
  private idempotencyKey = '';
  private pendingOperation = '';
  private inputs = new Map<string, InputState>();
  private inputSources = new WeakSet<object>();
  private inputTokens = new Map<string, InputState>();
  private outputs = new Map<string, OutputStream<unknown>>();
  private channels = new Map<number, { direction: 'input' | 'output'; token: string }>();
  private stableMappings = new Map<
    string,
    { direction: 'input' | 'output'; provisionalRef?: string; schemaIdentity?: string }
  >();
  private cursors = new Map<string, string>();
  private resultResolve?: (value: unknown) => void;
  private resultReject?: (error: unknown) => void;
  private reconnecting = false;
  private resultSeen = false;
  private finished = false;
  private queuedOutputBytes = 0;
  private pendingInputBytes = 0;
  private outputCancelReasons = new Map<string, 'consumer-drop'>();

  constructor(
    private readonly server: GolemServer,
    private readonly descriptor: StreamingInvocationDescriptor,
    private readonly signal?: AbortSignal,
  ) {}

  async start<R>(
    encode: (register: InputRegistrar) => PublicValue,
    decode: (value: unknown, resolve: OutputResolver) => R,
  ): Promise<R> {
    throwIfAborted(this.signal);
    let parameters: PublicValue;
    try {
      parameters = encode((source, itemEncode, wireKind = 'json') => {
        if (typeof source !== 'object' || source === null || this.inputSources.has(source))
          throw new StreamingProtocolError(
            'stream-already-consumed',
            'input stream appears more than once',
          );
        if (this.inputs.size >= 4096)
          throw new StreamingProtocolError('resource-exhausted', 'too many input streams');
        this.inputSources.add(source);
        const provisionalRef = crypto.randomUUID();
        this.inputs.set(provisionalRef, {
          source: source[Symbol.asyncIterator](),
          encode: itemEncode,
          sequence: 0n,
          terminal: false,
          wireKind,
        });
        return { $stream: { provisionalRef } };
      });
      for (const [name, value] of [
        ['application', this.descriptor.application],
        ['environment', this.descriptor.environment],
        ['agent type', this.descriptor.agentType],
        ['method', this.descriptor.method],
      ] as const) {
        if (typeof value !== 'string' || value.length === 0)
          throw new StreamingProtocolError('validation-error', `invalid ${name}`);
      }
      if (
        this.descriptor.phantomId !== undefined &&
        (typeof this.descriptor.phantomId !== 'string' ||
          !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
            this.descriptor.phantomId,
          ))
      )
        throw new StreamingProtocolError('validation-error', 'invalid phantom id');
      this.idempotencyKey = this.descriptor.idempotencyKey ?? crypto.randomUUID();
      if (
        typeof this.idempotencyKey !== 'string' ||
        this.idempotencyKey.length === 0 ||
        Buffer.byteLength(this.idempotencyKey) > 1024
      )
        throw new StreamingProtocolError('validation-error', 'invalid idempotency key');
      if (!Array.isArray(this.descriptor.config) || this.descriptor.config.length > 4096)
        throw new StreamingProtocolError('validation-error', 'invalid configuration');
      for (const entry of this.descriptor.config) {
        if (
          !isRecord(entry) ||
          !Array.isArray(entry.path) ||
          entry.path.length === 0 ||
          entry.path.some((part) => typeof part !== 'string' || part.length === 0)
        )
          throw new StreamingProtocolError('validation-error', 'invalid configuration path');
      }
    } catch (error) {
      this.releaseInputs();
      throw error;
    }
    this.pendingOperation = canonicalJson({
      version: 1,
      type: 'invocationStart',
      attemptId: this.attemptId,
      selector: {
        application: this.descriptor.application,
        environment: this.descriptor.environment,
        agentType: this.descriptor.agentType,
        constructorParameters: this.descriptor.constructorParameters,
        method: this.descriptor.method,
        ...(this.descriptor.phantomId === undefined
          ? {}
          : { phantomId: this.descriptor.phantomId }),
      },
      config: this.descriptor.config,
      methodParameters: parameters,
      idempotencyKey: this.idempotencyKey,
    });
    const result = new Promise<unknown>((resolve, reject) => {
      this.resultResolve = resolve;
      this.resultReject = reject;
    });
    this.signal?.addEventListener('abort', () => this.cancelAll(), { once: true });
    try {
      await this.connectRetry();
    } catch (error) {
      this.fail(error);
      await result.catch(() => undefined);
      throw error;
    }
    try {
      return decode(await result, (token, itemDecode, schemaIdentity, wireKind) =>
        this.output(token, itemDecode, schemaIdentity, wireKind),
      ) as R;
    } catch (error) {
      this.fail(error);
      this.socket?.close(1002);
      throw error;
    }
  }

  private endpoint(): { url: string; token: string } {
    const base =
      this.server.type === 'local'
        ? 'http://localhost:9881'
        : this.server.type === 'cloud'
          ? 'https://release.api.golem.cloud'
          : this.server.url;
    const token = this.server.type === 'local' ? LOCAL_WELL_KNOWN_TOKEN : this.server.token;
    const url = new URL(STREAM_ENDPOINT, base);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    return { url: url.toString(), token };
  }

  private connect(): Promise<void> {
    const { url, token } = this.endpoint();
    return new Promise((resolve, reject) => {
      let operationSent = false;
      let definitiveFailure = false;
      let settled = false;
      const resolveOnce = () => {
        if (!settled) {
          settled = true;
          operationSent = true;
          resolve();
        }
      };
      const rejectOnce = (error: unknown) => {
        if (!settled) {
          settled = true;
          reject(error);
        }
      };
      const socket = new WebSocket(url, STREAM_SUBPROTOCOL, {
        headers: { Authorization: `Bearer ${token}` },
        perMessageDeflate: false,
        maxPayload: 32 * 1024 * 1024,
      });
      this.socket = socket;
      socket.binaryType = 'nodebuffer';
      socket.once('open', () => {
        if (socket.protocol !== STREAM_SUBPROTOCOL) {
          definitiveFailure = true;
          socket.close(1002);
          rejectOnce(
            new StreamingProtocolError(
              'unsupported-subprotocol',
              'server selected an unsupported subprotocol',
            ),
          );
          return;
        }
        try {
          operationSent = true;
          socket.send(this.pendingOperation);
          resolveOnce();
        } catch (error) {
          operationSent = false;
          rejectOnce(
            new RetryableStreamingConnectionError(
              error instanceof Error ? error.message : 'failed to send attachment',
            ),
          );
        }
      });
      socket.on('message', (data, isBinary) => this.message(data, isBinary));
      socket.once('unexpected-response', (_request, response) => {
        definitiveFailure = true;
        response.resume();
        rejectOnce(
          new StreamingProtocolError(
            'handshake-failed',
            `WebSocket upgrade failed with HTTP ${response.statusCode ?? 'error'}`,
          ),
        );
      });
      socket.on('error', (error) => {
        if (!operationSent && !definitiveFailure)
          rejectOnce(new RetryableStreamingConnectionError(error.message));
      });
      socket.on('close', () => {
        if (!operationSent) {
          if (!definitiveFailure)
            rejectOnce(new RetryableStreamingConnectionError('connection closed before attach'));
          return;
        }
        if (this.socket !== socket || this.signal?.aborted || this.finished) return;
        void this.reconnect();
      });
    });
  }

  private async connectRetry(): Promise<void> {
    for (;;) {
      try {
        await this.connect();
        return;
      } catch (error) {
        if (!(error instanceof RetryableStreamingConnectionError) || this.signal?.aborted)
          throw error;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    }
  }

  private message(data: RawData, binary: boolean): void {
    try {
      if (binary) this.binary(Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer));
      else this.text(Buffer.isBuffer(data) ? data.toString('utf8') : String(data));
    } catch (error) {
      this.fail(error);
      this.socket?.close(1002);
    }
  }

  private text(text: string): void {
    const parsed = parseStrictJson(text);
    if (!isRecord(parsed))
      throw new StreamingProtocolError('malformed-message', 'invalid server message');
    const message = parsed as Record<string, any>;
    if (typeof message.version !== 'number' || typeof message.type !== 'string')
      throw new StreamingProtocolError('malformed-message', 'invalid server message');
    if (message.version !== 1)
      throw new StreamingProtocolError('unsupported-version', 'unsupported server message version');
    if (
      !this.accepted &&
      message.type !== 'invocationAccepted' &&
      message.type !== 'invocationRejected'
    )
      throw new StreamingProtocolError('protocol-error', 'message received before acceptance');
    if (this.finished)
      throw new StreamingProtocolError(
        'protocol-error',
        'message received after invocation finish',
      );
    switch (message.type) {
      case 'invocationAccepted': {
        if (this.accepted)
          throw new StreamingProtocolError('protocol-error', 'duplicate invocation acceptance');
        object(message, [
          'version',
          'type',
          'attemptId',
          'idempotencyKey',
          'mappings',
          'sessionToken',
        ]);
        if (message.attemptId !== this.attemptId)
          throw new StreamingProtocolError('protocol-error', 'unexpected attempt acceptance');
        if (message.idempotencyKey !== this.idempotencyKey)
          throw new StreamingProtocolError('protocol-error', 'unexpected invocation identity');
        validateOpaqueToken(message.sessionToken, 'session token');
        this.install(message.mappings, true);
        this.sessionToken = message.sessionToken;
        this.accepted = true;
        this.everAccepted = true;
        this.pendingResume = false;
        break;
      }
      case 'invocationRejected': {
        object(message, ['version', 'type', 'code', 'message', 'retryable'], ['attemptId']);
        if (
          (message.attemptId !== undefined && message.attemptId !== this.attemptId) ||
          (this.everAccepted && message.attemptId === undefined)
        )
          throw new StreamingProtocolError('protocol-error', 'unexpected attempt rejection');
        if (
          typeof message.code !== 'string' ||
          typeof message.message !== 'string' ||
          typeof message.retryable !== 'boolean'
        )
          throw new StreamingProtocolError('malformed-message', 'invalid invocation rejection');
        if (this.everAccepted && message.retryable) void this.reconnect(true);
        else this.fail(new StreamingProtocolError(message.code, message.message));
        break;
      }
      case 'invocationResult':
        if (this.resultSeen)
          throw new StreamingProtocolError('protocol-error', 'duplicate invocation result');
        object(message, ['version', 'type', 'mappings', 'result']);
        this.install(message.mappings);
        object(message.result, message.result.kind === 'value' ? ['kind', 'value'] : ['kind']);
        if (message.result.kind !== 'none' && message.result.kind !== 'value')
          throw new StreamingProtocolError('malformed-message', 'invalid invocation result');
        this.resultSeen = true;
        this.resultResolve?.(message.result.kind === 'none' ? undefined : message.result.value);
        break;
      case 'inputStreamAck':
        object(message, [
          'version',
          'type',
          'channel',
          'highestContiguousSequence',
          'mappings',
          'terminal',
        ]);
        this.install(message.mappings);
        if (typeof message.terminal !== 'boolean')
          throw new StreamingProtocolError('malformed-message', 'invalid input terminal state');
        this.ack(
          message.channel,
          decimal(message.highestContiguousSequence, 'highestContiguousSequence'),
          message.terminal,
        );
        break;
      case 'outputStreamItem': {
        object(message, [
          'version',
          'type',
          'channel',
          'cursorToken',
          'mappings',
          'sequence',
          'value',
        ]);
        this.install(message.mappings);
        const output = this.outputByChannel(message.channel);
        const sequence = decimal(message.sequence, 'sequence');
        const deliveredSequence = output.stream.accept(sequence, 1n, 'json');
        validateOpaqueToken(message.cursorToken, 'cursor token');
        output.stream.push(
          message.value,
          { token: message.cursorToken, sequence },
          Buffer.byteLength(text),
          deliveredSequence,
        );
        break;
      }
      case 'outputStreamEnd': {
        object(message, ['version', 'type', 'channel', 'outcome', 'sequence'], ['cursorToken']);
        const output = this.outputByChannel(message.channel);
        const sequence = decimal(message.sequence, 'sequence');
        const deliveredSequence = output.stream.accept(sequence, 0n);
        if (!isRecord(message.outcome) || typeof message.outcome.kind !== 'string')
          throw new StreamingProtocolError('malformed-message', 'invalid output outcome');
        let failure: StreamingProtocolError | undefined;
        if (message.outcome.kind === 'ok') object(message.outcome, ['kind']);
        else if (message.outcome.kind === 'error') {
          object(message.outcome, ['kind', 'code', 'message']);
          if (
            typeof message.outcome.code !== 'string' ||
            typeof message.outcome.message !== 'string'
          )
            throw new StreamingProtocolError('malformed-message', 'invalid output error');
          failure = new StreamingProtocolError(message.outcome.code, message.outcome.message);
        } else if (message.outcome.kind === 'cancelled') {
          object(message.outcome, ['kind', 'reason']);
          if (typeof message.outcome.reason !== 'string')
            throw new StreamingProtocolError('malformed-message', 'invalid cancellation reason');
          failure = new StreamingProtocolError('cancelled', message.outcome.reason);
        } else throw new StreamingProtocolError('malformed-message', 'invalid output outcome');
        const cursor =
          message.cursorToken !== undefined
            ? (validateOpaqueToken(message.cursorToken, 'cursor token'),
              {
                token: message.cursorToken as string,
                sequence,
              })
            : undefined;
        if (failure === undefined) output.stream.finish(cursor, deliveredSequence);
        else output.stream.fail(failure, cursor, deliveredSequence);
        this.outputCancelReasons.delete(output.token);
        break;
      }
      case 'streamCancel':
        object(message, ['version', 'type', 'channel', 'reason']);
        if (typeof message.reason !== 'string')
          throw new StreamingProtocolError('malformed-message', 'invalid cancellation reason');
        this.cancelChannel(message.channel);
        break;
      case 'attachmentRevoked':
        object(message, ['version', 'type', 'reason']);
        if (message.reason !== 'replaced')
          throw new StreamingProtocolError('malformed-message', 'invalid revocation reason');
        void this.reconnect(true);
        break;
      case 'invocationFinished':
        object(message, ['version', 'type', 'outcome']);
        if (!isRecord(message.outcome) || typeof message.outcome.kind !== 'string')
          throw new StreamingProtocolError('malformed-message', 'invalid invocation outcome');
        if (message.outcome.kind === 'failure') {
          object(message.outcome, ['kind', 'code', 'message']);
          if (
            typeof message.outcome.code !== 'string' ||
            typeof message.outcome.message !== 'string'
          )
            throw new StreamingProtocolError('malformed-message', 'invalid invocation failure');
          if ([...this.outputs.values()].some((output) => !output.isTerminal()))
            throw new StreamingProtocolError(
              'protocol-error',
              'invocation failure preceded output terminal',
            );
          this.finish(new StreamingProtocolError(message.outcome.code, message.outcome.message));
        } else if (message.outcome.kind !== 'success')
          throw new StreamingProtocolError('malformed-message', 'invalid invocation outcome');
        else {
          object(message.outcome, ['kind']);
          if (!this.resultSeen)
            throw new StreamingProtocolError('protocol-error', 'invocation finished before result');
          if ([...this.outputs.values()].some((output) => !output.isTerminal()))
            throw new StreamingProtocolError(
              'protocol-error',
              'invocation finished before output terminal',
            );
          this.finish();
        }
        this.socket?.close(1000);
        break;
      default:
        throw new StreamingProtocolError(
          'malformed-message',
          `unknown server message ${message.type}`,
        );
    }
  }

  private binary(frame: Buffer): void {
    if (frame.length < 4)
      throw new StreamingProtocolError('malformed-message', 'truncated binary frame');
    const length = frame.readUInt32BE(0);
    if (length > 16 * 1024 || length + 4 > frame.length)
      throw new StreamingProtocolError('malformed-message', 'invalid binary metadata length');
    const metadata = object(
      parseStrictJson(frame.subarray(4, 4 + length).toString('utf8')),
      ['version', 'kind', 'channel', 'sequence', 'itemCount'],
      ['cursorToken', 'mimeType'],
    );
    if (typeof metadata.version !== 'number')
      throw new StreamingProtocolError('malformed-message', 'invalid binary metadata version');
    if (metadata.version !== 1)
      throw new StreamingProtocolError(
        'unsupported-version',
        'unsupported binary metadata version',
      );
    const payload = frame.subarray(4 + length);
    const output = this.outputByChannel(metadata.channel);
    const sequence = decimal(metadata.sequence, 'sequence');
    const itemCount = decimal(metadata.itemCount, 'itemCount');
    validateOpaqueToken(metadata.cursorToken, 'cursor token');
    if (metadata.kind === 'output-u8') {
      if (
        payload.length === 0 ||
        itemCount !== BigInt(payload.length) ||
        payload.length > 1024 * 1024 ||
        metadata.mimeType !== undefined
      )
        throw new StreamingProtocolError('malformed-message', 'packed u8 item count mismatch');
      if (sequence + itemCount > U64_MAX)
        throw new StreamingProtocolError('invalid-sequence', 'packed u8 sequence overflow');
      const deliveredSequence = output.stream.accept(sequence, itemCount, 'u8');
      output.stream.pushPacked(
        payload,
        { token: metadata.cursorToken, sequence: sequence + itemCount - 1n },
        deliveredSequence,
      );
    } else if (metadata.kind === 'output-binary') {
      if (
        itemCount !== 1n ||
        payload.length > MAX_LOGICAL_VALUE_BYTES ||
        (metadata.mimeType !== undefined && typeof metadata.mimeType !== 'string')
      )
        throw new StreamingProtocolError('malformed-message', 'invalid binary item metadata');
      const deliveredSequence = output.stream.accept(sequence, 1n, 'binary');
      output.stream.push(
        { bytes: payload.toString('base64'), mimeType: metadata.mimeType },
        { token: metadata.cursorToken, sequence },
        payload.length + (metadata.mimeType ? Buffer.byteLength(metadata.mimeType) : 0),
        deliveredSequence,
      );
    } else throw new StreamingProtocolError('malformed-message', 'invalid server binary kind');
  }

  private install(mappings: any[], complete = false): void {
    if (!Array.isArray(mappings) || mappings.length > 4096)
      throw new StreamingProtocolError('malformed-message', 'invalid stream mappings');
    const messageChannels = new Set<number>();
    const messageTokens = new Set<string>();
    const messageProvisionals = new Set<string>();
    const parsed: Array<{
      channel: number;
      direction: 'input' | 'output';
      token: string;
      provisionalRef?: string;
      input?: InputState;
      highWater?: bigint;
      terminal?: boolean;
    }> = [];
    for (const mapping of mappings) {
      object(
        mapping,
        ['channel', 'direction', 'streamToken'],
        ['inputHighWater', 'provisionalRef'],
      );
      validateChannel(mapping.channel);
      if (
        (mapping.direction !== 'input' && mapping.direction !== 'output') ||
        (mapping.direction === 'input') !== (mapping.inputHighWater !== undefined)
      )
        throw new StreamingProtocolError('stream-conflict', 'invalid stream mapping');
      validateOpaqueToken(mapping.streamToken, 'stream token');
      if (messageChannels.has(mapping.channel) || messageTokens.has(mapping.streamToken))
        throw new StreamingProtocolError('stream-conflict', 'duplicate stream mapping');
      messageChannels.add(mapping.channel);
      messageTokens.add(mapping.streamToken);
      if (mapping.provisionalRef !== undefined) {
        if (
          mapping.direction !== 'input' ||
          typeof mapping.provisionalRef !== 'string' ||
          !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
            mapping.provisionalRef,
          ) ||
          messageProvisionals.has(mapping.provisionalRef)
        )
          throw new StreamingProtocolError('stream-conflict', 'invalid provisional stream mapping');
        messageProvisionals.add(mapping.provisionalRef);
      }
      const connection = this.channels.get(mapping.channel);
      if (
        connection &&
        (connection.direction !== mapping.direction || connection.token !== mapping.streamToken)
      )
        throw new StreamingProtocolError('stream-conflict', 'channel mapping rebound');
      if (
        [...this.channels].some(
          ([channel, value]) => channel !== mapping.channel && value.token === mapping.streamToken,
        )
      )
        throw new StreamingProtocolError('stream-conflict', 'stream mapping rebound');
      const stable = this.stableMappings.get(mapping.streamToken);
      if (
        stable &&
        (stable.direction !== mapping.direction ||
          (stable.provisionalRef !== undefined &&
            mapping.provisionalRef !== undefined &&
            stable.provisionalRef !== mapping.provisionalRef))
      )
        throw new StreamingProtocolError('stream-conflict', 'stable stream mapping rebound');
      if (!stable && this.stableMappings.size >= 4096)
        throw new StreamingProtocolError('resource-exhausted', 'too many stream mappings');
      let input: InputState | undefined;
      let highWater: bigint | undefined;
      let terminal: boolean | undefined;
      if (mapping.direction === 'input') {
        const provisionalInput =
          mapping.provisionalRef === undefined
            ? undefined
            : this.inputs.get(mapping.provisionalRef);
        if (mapping.provisionalRef !== undefined && !provisionalInput)
          throw new StreamingProtocolError('stream-conflict', 'unknown provisional stream mapping');
        input = provisionalInput ?? this.inputTokens.get(mapping.streamToken);
        if (!input)
          throw new StreamingProtocolError('stream-conflict', 'unknown provisional stream mapping');
        const previous = this.inputTokens.get(mapping.streamToken);
        if (previous && previous !== input)
          throw new StreamingProtocolError('stream-conflict', 'input stream token was rebound');
        if (
          [...this.inputTokens].some(
            ([token, known]) => token !== mapping.streamToken && known === input,
          )
        )
          throw new StreamingProtocolError('stream-conflict', 'input stream was rebound');
        object(mapping.inputHighWater, ['sequence', 'terminal']);
        highWater = decimal(mapping.inputHighWater.sequence, 'sequence');
        terminal = mapping.inputHighWater.terminal;
        if (typeof terminal !== 'boolean')
          throw new StreamingProtocolError('stream-conflict', 'invalid input high-water');
        this.validateInputReconcile(input, highWater, terminal);
      } else if (mapping.provisionalRef !== undefined) {
        throw new StreamingProtocolError('stream-conflict', 'output mapping has provisional input');
      }
      parsed.push({
        channel: mapping.channel,
        direction: mapping.direction,
        token: mapping.streamToken,
        provisionalRef: mapping.provisionalRef,
        input,
        highWater,
        terminal,
      });
    }
    if (new Set([...this.stableMappings.keys(), ...messageTokens]).size > 4096)
      throw new StreamingProtocolError('resource-exhausted', 'too many stream mappings');
    if (complete) {
      if (this.pendingResume) {
        if ([...this.stableMappings.keys()].some((token) => !messageTokens.has(token)))
          throw new StreamingProtocolError(
            'stream-conflict',
            'resume acceptance omitted a known stream mapping',
          );
      } else {
        const expected = new Set(this.inputs.keys());
        if (
          expected.size !== messageProvisionals.size ||
          [...expected].some((reference) => !messageProvisionals.has(reference))
        )
          throw new StreamingProtocolError(
            'stream-conflict',
            'initial acceptance omitted a provisional stream mapping',
          );
      }
    }
    const pumps = new Set<InputState>();
    for (const mapping of parsed) {
      const stable = this.stableMappings.get(mapping.token);
      this.stableMappings.set(mapping.token, {
        direction: mapping.direction,
        provisionalRef: mapping.provisionalRef ?? stable?.provisionalRef,
        schemaIdentity: stable?.schemaIdentity,
      });
      this.channels.set(mapping.channel, {
        direction: mapping.direction,
        token: mapping.token,
      });
      if (mapping.direction === 'output') {
        this.ensureOutput(mapping.token);
        const reason = this.outputCancelReasons.get(mapping.token);
        if (reason)
          this.send({ version: 1, type: 'streamCancel', channel: mapping.channel, reason });
      } else {
        const input = mapping.input as InputState;
        this.inputTokens.set(mapping.token, input);
        input.channel = mapping.channel;
        this.reconcileInput(input, mapping.highWater as bigint, mapping.terminal as boolean);
        if (mapping.terminal) input.cancelReason = undefined;
        else if (input.cancelReason)
          this.send({
            version: 1,
            type: 'streamCancel',
            channel: mapping.channel,
            reason: input.cancelReason,
          });
        else if (input.pending) this.socket?.send(input.pending.render(mapping.channel));
        else pumps.add(input);
      }
    }
    pumps.forEach((input) => void this.pump(input));
  }

  private async pump(input: InputState): Promise<void> {
    if (input.terminal || input.cancelReason || input.channel === undefined || input.pending)
      return;
    try {
      const next = input.naturalEnd
        ? ({ done: true } as IteratorResult<unknown>)
        : await this.pull(input);
      const pending = next.done
        ? this.terminalInput(input.sequence)
        : input.wireKind === 'u8'
          ? await this.packedU8Input(input, next.value)
          : input.wireKind === 'binary'
            ? this.binaryInput(input, next.value)
            : this.jsonInput(input, next.value);
      const channel = input.channel;
      const message = pending.render(channel ?? 4294967295);
      const bytes = Buffer.byteLength(message);
      if (bytes > MAX_LOGICAL_VALUE_BYTES)
        throw new StreamingProtocolError(
          'resource-exhausted',
          'input logical value limit exceeded',
        );
      if (this.pendingInputBytes + bytes > MAX_QUEUE_BYTES)
        throw new StreamingProtocolError(
          'resource-exhausted',
          'unacknowledged input limit exceeded',
        );
      pending.bytes = bytes;
      input.pending = pending;
      this.pendingInputBytes += bytes;
      if (channel !== undefined) this.socket?.send(pending.render(channel));
    } catch (error) {
      if (error instanceof StreamingProtocolError) {
        this.fail(error);
        this.socket?.close(1002);
        return;
      }
      input.cancelReason = 'source-unavailable';
      if (input.channel !== undefined)
        this.send({
          version: 1,
          type: 'streamCancel',
          channel: input.channel,
          reason: input.cancelReason,
        });
    }
  }

  private pull(input: InputState): Promise<IteratorResult<unknown>> {
    const pending = input.bufferedPull;
    if (pending) {
      input.bufferedPull = undefined;
      return pending;
    }
    return input.source.next();
  }

  private jsonInput(input: InputState, value: unknown): PendingInput {
    const encoded = input.encode(value);
    const sequence = input.sequence;
    return {
      sequence,
      itemCount: 1n,
      terminal: false,
      bytes: 0,
      render: (channel) =>
        canonicalJson({
          version: 1,
          type: 'inputStreamItem',
          channel,
          sequence: sequence.toString(),
          value: encoded,
        }),
    };
  }

  private binaryInput(input: InputState, value: unknown): PendingInput {
    const encoded = input.encode(value) as any;
    if (!isRecord(encoded) || (!('bytes' in encoded) && encoded.bytes === undefined))
      throw new StreamingProtocolError('schema-mismatch', 'invalid binary stream item');
    let payload: Buffer;
    if (typeof encoded.bytes === 'string') {
      if (
        encoded.bytes.length % 4 !== 0 ||
        !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(encoded.bytes)
      )
        throw new StreamingProtocolError(
          'schema-mismatch',
          'binary bytes are not canonical base64',
        );
      payload = Buffer.from(encoded.bytes, 'base64');
      if (payload.toString('base64') !== encoded.bytes)
        throw new StreamingProtocolError(
          'schema-mismatch',
          'binary bytes are not canonical base64',
        );
    } else if (encoded.bytes instanceof Uint8Array) payload = Buffer.from(encoded.bytes);
    else throw new StreamingProtocolError('schema-mismatch', 'invalid binary stream bytes');
    if (payload.length > MAX_LOGICAL_VALUE_BYTES)
      throw new StreamingProtocolError('resource-exhausted', 'binary stream item exceeds 16 MiB');
    const sequence = input.sequence;
    const mimeType = encoded.mimeType;
    if (
      mimeType !== undefined &&
      (typeof mimeType !== 'string' ||
        !/^[A-Za-z0-9!#$&^_.+\-]+\/[A-Za-z0-9!#$&^_.+\-]+$/u.test(mimeType))
    )
      throw new StreamingProtocolError('schema-mismatch', 'invalid binary MIME type');
    return {
      sequence,
      itemCount: 1n,
      terminal: false,
      bytes: 0,
      render: (channel) =>
        this.binaryInputFrame('input-binary', channel, sequence, payload, mimeType),
    };
  }

  private async packedU8Input(input: InputState, first: unknown): Promise<PendingInput> {
    const bytes = [this.u8(input.encode(first))];
    while (bytes.length < 1024 * 1024) {
      const pull = input.source.next();
      const next = await Promise.race([
        pull.then((value) => ({ kind: 'value' as const, value })),
        new Promise<{ kind: 'timeout' }>((resolve) =>
          setTimeout(() => resolve({ kind: 'timeout' }), 1),
        ),
      ]);
      if (next.kind === 'timeout') {
        input.bufferedPull = pull;
        break;
      }
      if (next.value.done) {
        input.naturalEnd = true;
        break;
      }
      bytes.push(this.u8(input.encode(next.value.value)));
    }
    return this.packedU8Range(input.sequence, Buffer.from(bytes));
  }

  private packedU8Range(sequence: bigint, payload: Buffer): PendingInput {
    return {
      sequence,
      itemCount: BigInt(payload.length),
      terminal: false,
      bytes: 0,
      render: (channel) => this.binaryInputFrame('input-u8', channel, sequence, payload),
      trim: (accepted) => {
        const offset = Number(accepted - sequence);
        return this.packedU8Range(accepted, payload.subarray(offset));
      },
    };
  }

  private terminalInput(sequence: bigint): PendingInput {
    return {
      sequence,
      itemCount: 0n,
      terminal: true,
      bytes: 0,
      render: (channel) =>
        canonicalJson({
          version: 1,
          type: 'inputStreamEnd',
          channel,
          sequence: sequence.toString(),
        }),
    };
  }

  private binaryInputFrame(
    kind: 'input-u8' | 'input-binary',
    channel: number,
    sequence: bigint,
    payload: Buffer,
    mimeType?: string,
  ): Buffer {
    const metadata = {
      version: 1,
      kind,
      channel,
      sequence: sequence.toString(),
      itemCount: kind === 'input-u8' ? payload.length.toString() : '1',
      ...(kind === 'input-binary' && mimeType ? { mimeType } : {}),
    };
    return encodeStreamSessionBinaryEnvelope(metadata, payload);
  }

  private u8(value: unknown): number {
    if (!Number.isInteger(value) || (value as number) < 0 || (value as number) > 255)
      throw new StreamingProtocolError('schema-mismatch', 'u8 stream item is out of range');
    return value as number;
  }

  private validateInputReconcile(input: InputState, highWater: bigint, terminal: boolean): void {
    if (highWater < input.sequence)
      throw new StreamingProtocolError('invalid-sequence', 'input high-water moved backwards');
    const pending = input.pending;
    if (!pending) {
      if (
        highWater !== input.sequence ||
        (terminal !== input.terminal && !(terminal && input.cancelReason !== undefined))
      )
        throw new StreamingProtocolError(
          'invalid-sequence',
          'input high-water conflicts with local state',
        );
      return;
    }
    const end = pending.sequence + pending.itemCount;
    if (highWater < pending.sequence || highWater > end)
      throw new StreamingProtocolError(
        'invalid-sequence',
        'input high-water conflicts with pending data',
      );
    if (terminal && !pending.terminal)
      throw new StreamingProtocolError(
        'invalid-sequence',
        'input high-water reports an unsent terminal',
      );
    if (!pending.terminal && highWater > pending.sequence && highWater < end && !pending.trim)
      throw new StreamingProtocolError('invalid-sequence', 'input high-water split an atomic item');
  }

  private reconcileInput(input: InputState, highWater: bigint, terminal: boolean): void {
    this.validateInputReconcile(input, highWater, terminal);
    const pending = input.pending;
    if (!pending) {
      if (terminal && input.cancelReason) {
        input.cancelReason = undefined;
        input.terminal = true;
      }
      return;
    }
    const end = pending.sequence + pending.itemCount;
    if (pending.terminal) {
      if (highWater !== pending.sequence || !terminal) return;
      this.clearPendingInput(input);
      input.terminal = true;
      return;
    }
    if (highWater === end) this.clearPendingInput(input);
    else if (highWater > pending.sequence) {
      const trimmed = (pending.trim as (accepted: bigint) => PendingInput)(highWater);
      this.pendingInputBytes -= pending.bytes;
      const rendered = trimmed.render(input.channel as number);
      trimmed.bytes = Buffer.byteLength(rendered);
      this.pendingInputBytes += trimmed.bytes;
      input.pending = trimmed;
    }
    input.sequence = highWater;
  }

  private ack(channel: number, highWater: bigint, terminal: boolean): void {
    validateChannel(channel);
    const input = [...this.inputs.values()].find((candidate) => candidate.channel === channel);
    if (!input)
      throw new StreamingProtocolError('invalid-channel', 'ACK referenced an unknown input');
    if (highWater < input.sequence)
      throw new StreamingProtocolError('invalid-sequence', 'input ACK moved backwards');
    if (input.pending && highWater > input.pending.sequence + input.pending.itemCount)
      throw new StreamingProtocolError('invalid-sequence', 'input ACK advanced beyond sent data');
    this.reconcileInput(input, highWater, terminal);
    if (!input.pending) void this.pump(input);
  }

  private output(
    token: string,
    decode: (value: unknown) => unknown,
    schemaIdentity = 'anonymous',
    wireKind: 'json' | 'u8' | 'binary' = 'json',
  ): AgentStream<unknown> {
    validateOpaqueToken(token, 'stream token');
    const stable = this.stableMappings.get(token);
    if (!stable || stable.direction !== 'output')
      throw new StreamingProtocolError('stream-conflict', 'unknown output stream token');
    if (stable.schemaIdentity && stable.schemaIdentity !== schemaIdentity)
      throw new StreamingProtocolError('stream-conflict', 'output stream schema changed');
    stable.schemaIdentity = schemaIdentity;
    let stream = this.outputs.get(token);
    if (!stream) {
      stream = new OutputStream(
        () => {
          this.outputCancelReasons.set(token, 'consumer-drop');
          const channel = [...this.channels].find(([, value]) => value.token === token)?.[0];
          if (channel !== undefined)
            this.send({ version: 1, type: 'streamCancel', channel, reason: 'consumer-drop' });
        },
        (cursor, deliveredSequence) => {
          const previous = (stream as any).__delivered as
            | { token: string; sequence: bigint; deliveredSequence: bigint }
            | undefined;
          if (previous && cursor.sequence < previous.sequence)
            throw new StreamingProtocolError('invalid-sequence', 'output delivery moved backwards');
          if (previous && cursor.sequence === previous.sequence && cursor.token !== previous.token)
            throw new StreamingProtocolError('invalid-cursor', 'conflicting output cursor');
          if (
            [...this.cursors].some(
              ([knownToken, knownCursor]) => knownToken !== token && knownCursor === cursor.token,
            )
          )
            throw new StreamingProtocolError(
              'invalid-cursor',
              'cursor token reused across streams',
            );
          (stream as any).__delivered = { ...cursor, deliveredSequence };
          this.cursors.set(token, cursor.token);
        },
        (delta) => {
          const next = this.queuedOutputBytes + delta;
          if (next > MAX_SESSION_QUEUE_BYTES || next < 0)
            throw new StreamingProtocolError(
              'resource-exhausted',
              'session output delivery queue limit exceeded',
            );
          this.queuedOutputBytes = next;
        },
        (error) => {
          this.fail(error);
          this.socket?.close(1002);
        },
      );
      this.outputs.set(token, stream);
    }
    stream.expose(wireKind);
    stream.setDecoder(decode);
    return stream;
  }

  private ensureOutput(token: string): OutputStream<unknown> {
    let stream = this.outputs.get(token);
    if (!stream) {
      stream = new OutputStream(
        () => {
          this.outputCancelReasons.set(token, 'consumer-drop');
          const channel = [...this.channels].find(([, value]) => value.token === token)?.[0];
          if (channel !== undefined)
            this.send({ version: 1, type: 'streamCancel', channel, reason: 'consumer-drop' });
        },
        (cursor, deliveredSequence) => {
          const previous = (stream as any).__delivered as
            | { token: string; sequence: bigint; deliveredSequence: bigint }
            | undefined;
          if (previous && cursor.sequence < previous.sequence)
            throw new StreamingProtocolError('invalid-sequence', 'output delivery moved backwards');
          if (previous && cursor.sequence === previous.sequence && cursor.token !== previous.token)
            throw new StreamingProtocolError('invalid-cursor', 'conflicting output cursor');
          (stream as any).__delivered = { ...cursor, deliveredSequence };
          this.cursors.set(token, cursor.token);
        },
        (delta) => {
          const next = this.queuedOutputBytes + delta;
          if (next > MAX_SESSION_QUEUE_BYTES || next < 0)
            throw new StreamingProtocolError(
              'resource-exhausted',
              'session output delivery queue limit exceeded',
            );
          this.queuedOutputBytes = next;
        },
        (error) => {
          this.fail(error);
          this.socket?.close(1002);
        },
      );
      this.outputs.set(token, stream);
    }
    return stream;
  }

  private outputByChannel(channel: number): {
    token: string;
    stream: OutputStream<unknown>;
  } {
    validateChannel(channel);
    const mapping = this.channels.get(channel);
    if (!mapping || mapping.direction !== 'output')
      throw new StreamingProtocolError('invalid-channel', 'unknown output channel');
    const stream = this.ensureOutput(mapping.token);
    return { token: mapping.token, stream };
  }

  private async reconnect(forceFreshResume = false): Promise<void> {
    if (this.reconnecting || this.signal?.aborted) return;
    this.reconnecting = true;
    try {
      const freshResume = forceFreshResume || this.accepted;
      const previous = this.socket;
      this.socket = undefined;
      previous?.terminate();
      this.channels.clear();
      for (const input of this.inputs.values()) input.channel = undefined;
      if (this.everAccepted) for (const output of this.outputs.values()) output.prepareResume();
      if (freshResume) {
        if (!this.sessionToken)
          throw new StreamingProtocolError('stale-session', 'accepted session has no resume token');
        const outputCursors = [...this.cursors.values()];
        if (new Set(outputCursors).size !== outputCursors.length)
          throw new StreamingProtocolError('invalid-cursor', 'duplicate output cursor token');
        this.attemptId = crypto.randomUUID();
        this.pendingResume = true;
        this.pendingOperation = canonicalJson({
          version: 1,
          type: 'resumeAttach',
          attemptId: this.attemptId,
          operation: 'resume',
          sessionToken: this.sessionToken,
          outputCursors,
        });
      }
      this.accepted = false;
      await this.connectRetry();
    } catch (error) {
      this.fail(error);
    } finally {
      this.reconnecting = false;
    }
  }

  private cancelChannel(channel: number): void {
    validateChannel(channel);
    const mapping = this.channels.get(channel);
    if (!mapping || mapping.direction !== 'input')
      throw new StreamingProtocolError(
        'invalid-channel',
        'cancellation referenced an unknown input',
      );
    const input = this.inputTokens.get(mapping.token);
    if (!input)
      throw new StreamingProtocolError(
        'invalid-channel',
        'cancellation referenced an unknown input',
      );
    void input.source.return?.();
    this.clearPendingInput(input);
    input.cancelReason = undefined;
    input.terminal = true;
  }
  private clearPendingInput(input: InputState): void {
    if (input.pending) this.pendingInputBytes -= input.pending.bytes;
    input.pending = undefined;
  }
  private cancelAll(): void {
    this.fail(this.signal?.reason ?? new DOMException('The operation was aborted.', 'AbortError'));
    this.socket?.close(1000);
  }
  private send(message: unknown): void {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(canonicalJson(message));
  }
  private fail(error: unknown): void {
    this.finished = true;
    this.resultReject?.(error);
    for (const output of this.outputs.values()) output.abort(error);
    this.releaseInputs();
  }
  private finish(error?: unknown): void {
    this.finished = true;
    if (error !== undefined) this.resultReject?.(error);
    this.releaseInputs();
  }
  private releaseInputs(): void {
    for (const input of this.inputs.values()) {
      if (!input.terminal) void input.source.return?.();
      this.clearPendingInput(input);
      input.terminal = true;
    }
  }
}
