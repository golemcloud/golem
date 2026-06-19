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
  mime_type?: string;
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
  | { kind: 's64'; value: number }
  | { kind: 'u8'; value: number }
  | { kind: 'u16'; value: number }
  | { kind: 'u32'; value: number }
  | { kind: 'u64'; value: number }
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
  | { kind: 'url'; value: { url: string } };

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
  methodName: string;
  methodParameters: SchemaValue;
  mode: AgentInvocationMode;
  scheduleAt?: string; // ISO 8601 datetime
  idempotencyKey?: IdempotencyKey;
}

export interface AgentInvocationResult {
  agentId: AgentId;
  result?: TypedSchemaValue;
  componentRevision?: number;
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
    body: JSON.stringify(request),
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
      body: JSON.stringify(request),
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
    if (allowedCodes.length > 0) {
      if (language === undefined) {
        throw new Error(`Language code is required. Allowed codes: ${allowedCodes.join(', ')}`);
      }
      if (!allowedCodes.includes(language)) {
        throw new Error(
          `Invalid value for parameter ${parameterName}. Language code \`${language}\` is not allowed. Allowed codes: ${allowedCodes.join(', ')}`,
        );
      }
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
    const mimeType = payload.value.mime_type ?? '';
    if (allowedMimeTypes.length > 0 && !allowedMimeTypes.includes(mimeType)) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Mime type \`${mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(', ')}`,
      );
    }
    return {
      tag: 'inline',
      val: new Uint8Array(payload.value.bytes),
      mimeType,
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
            mime_type: input.mimeType as string | undefined,
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
