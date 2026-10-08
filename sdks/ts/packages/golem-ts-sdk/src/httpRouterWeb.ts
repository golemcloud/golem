// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { AgentStream, disposeAgentStream } from './schema/agentStream';
import {
  copyHttpHeaders,
  HttpRouterError,
  type HttpHeader,
  type HttpRequest,
  type HttpResponse,
} from './httpRouterContract';

const responseHeaders = new WeakMap<
  Response,
  { headers: readonly HttpHeader[] } | { error: unknown }
>();

/** Replace the canonical header sequence; validation errors surface during owned conversion. */
export function withRawHeaders(response: Response, headers: readonly HttpHeader[]): Response {
  try {
    responseHeaders.set(response, { headers: copyHttpHeaders(headers) });
  } catch (error) {
    // Returning the response lets the invocation own and await its disposal on failure.
    responseHeaders.set(response, { error });
  }
  return response;
}

function byteString(value: Uint8Array): string {
  let result = '';
  for (const byte of value) result += String.fromCharCode(byte);
  return result;
}

function headerBytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code > 255) throw new HttpRouterError('invalid-header');
    bytes[i] = code;
  }
  return bytes;
}

/** @internal One owned request body; the Web view never prefetches. */
export async function webRequest(raw: HttpRequest<AgentStream<Uint8Array>>): Promise<{
  request: Request;
  close: () => Promise<void>;
}> {
  let closed = false;
  let cleanup: Promise<void> | undefined;
  let controller: ReadableStreamDefaultController<Uint8Array>;
  const close = () => {
    if (cleanup) return cleanup;
    closed = true;
    cleanup = (async () => {
      try {
        await disposeAgentStream(raw.body);
        try {
          controller.close();
        } catch {
          /* A cancelled Web reader is already closed. */
        }
      } catch (error) {
        controller.error(error);
        throw error;
      }
    })();
    return cleanup;
  };
  const body = new ReadableStream<Uint8Array>(
    {
      start(value) {
        controller = value;
      },
      async pull(controller) {
        try {
          const next = await raw.body.next();
          if (closed) return;
          if (next.done) {
            await close();
          } else controller.enqueue(next.value);
        } catch (error) {
          if (closed) return;
          controller.error(error);
          await close().catch(() => undefined);
        }
      },
      cancel: close,
    },
    { highWaterMark: 0 },
  );
  try {
    const headers = new Headers();
    for (const { name, value } of copyHttpHeaders(raw.headers))
      headers.append(name, byteString(value));
    headers.set('host', raw.authority);
    const url = `${raw.scheme}://${raw.authority}${raw.path}${raw.query === undefined ? '' : `?${raw.query}`}`;
    const init: RequestInit & { duplex: 'half' } = { method: raw.method, headers, duplex: 'half' };
    if (raw.method.toUpperCase() !== 'GET' && raw.method.toUpperCase() !== 'HEAD') init.body = body;
    return { request: new Request(url, init), close };
  } catch {
    // Constructors can include the URL or field value in their exception text.
    await close().catch(() => undefined);
    throw new HttpRouterError('web-request');
  }
}

/** @internal Move a Web body into the ordinary agent stream without eager reads. */
export function webResponse(
  response: Response,
  disposeInput: () => Promise<void>,
): HttpResponse<AgentStream<Uint8Array>> {
  const override = responseHeaders.get(response);
  if (override && 'error' in override) throw override.error;
  let headers = override?.headers;
  if (!headers) {
    const result: HttpHeader[] = [];
    for (const [name, value] of response.headers) {
      if (name !== 'set-cookie') result.push({ name, value: headerBytes(value) });
    }
    for (const value of response.headers.getSetCookie())
      result.push({ name: 'set-cookie', value: headerBytes(value) });
    headers = copyHttpHeaders(result);
  }
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  let closed = false;
  let cleanup: Promise<void> | undefined;
  const close = () => {
    if (cleanup) return cleanup;
    closed = true;
    cleanup = (async () => {
      try {
        if (reader) await reader.cancel();
        else await response.body?.cancel();
      } finally {
        reader?.releaseLock();
        await disposeInput();
      }
    })();
    return cleanup;
  };
  return {
    status: response.status,
    headers,
    body: AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]() {
        return {
          async next() {
            if (closed) return { done: true as const, value: undefined };
            try {
              reader ??= response.body?.getReader();
              const next = reader ? await reader.read() : { done: true as const, value: undefined };
              if (next.done) {
                await close();
                return { done: true as const, value: undefined };
              }
              if (!(next.value instanceof Uint8Array)) throw new HttpRouterError('web-body-chunk');
              return { done: false as const, value: next.value };
            } catch (error) {
              await close().catch(() => undefined);
              throw error;
            }
          },
          async return() {
            await close();
            return { done: true as const, value: undefined };
          },
        };
      },
    }),
  };
}
